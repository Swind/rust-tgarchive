use std::{
    fs, io,
    path::{Path, PathBuf},
};

use crate::infrastructure::persistence::{search_groups::SearchGroupStore, sqlite::SqliteStore};

const SIDECAR_ENV: &str = "TGARCHIVE_SEARCH_GROUPS_DB";

pub(super) async fn open_search_groups(archive: &SqliteStore) -> Option<SearchGroupStore> {
    let result = async {
        let filename = archive.database_filename().await?;
        let configured = std::env::var_os(SIDECAR_ENV).map(PathBuf::from);
        let path = sidecar_path(Path::new(&filename), configured).map_err(sqlx::Error::Io)?;
        SearchGroupStore::open(&path).await
    }
    .await;
    match result {
        Ok(store) => Some(store),
        Err(error) => {
            tracing::warn!(%error, "saved search groups unavailable; set {SIDECAR_ENV} to a separate writable database file");
            None
        }
    }
}

fn sidecar_path(archive: &Path, configured: Option<PathBuf>) -> io::Result<PathBuf> {
    let path = match configured {
        Some(path) => path,
        None if archive.file_name().is_some() => append_suffix(archive, ".search-groups.db"),
        None => return Err(io::Error::other("archive has no filename")),
    };
    let filename = path
        .file_name()
        .ok_or_else(|| io::Error::other("expected a sidecar filename"))?;
    let path = match fs::symlink_metadata(&path) {
        Ok(_) => {
            let meta = fs::metadata(&path)?;
            if !meta.is_file() {
                return Err(io::Error::other("sidecar must be a regular file"));
            }
            // SQLite journal filenames are path-based; hardlinked DB files cannot be used safely.
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                if meta.nlink() > 1 {
                    return Err(io::Error::other("sidecar cannot be a hardlinked file"));
                }
            }
            fs::canonicalize(&path)?
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let parent = path
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new("."));
            fs::canonicalize(parent)?.join(filename)
        }
        Err(error) => return Err(error),
    };
    if !archive.as_os_str().is_empty() {
        let archive = fs::canonicalize(archive)?;
        if path == archive
            || ["-journal", "-wal", "-shm"].iter().any(|suffix| {
                append_suffix(&archive, suffix) == path || append_suffix(&path, suffix) == archive
            })
        {
            return Err(io::Error::other(
                "sidecar path overlaps the archive database",
            ));
        }
    }
    Ok(path)
}

fn append_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut filename = path.as_os_str().to_owned();
    filename.push(suffix);
    filename.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn default_is_derived_from_the_real_sqlite_filename() {
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("archive one.db");
        let store = SqliteStore::connect(&format!(
            "sqlite://{}/archive%20one.db?mode=rwc",
            tmp.path().display()
        ))
        .await
        .unwrap();
        let db = fs::canonicalize(db).unwrap();
        let actual = store.database_filename().await.unwrap();
        assert_eq!(
            sidecar_path(Path::new(&actual), None).unwrap(),
            append_suffix(&db, ".search-groups.db")
        );
        for overlap in [
            db.clone(),
            append_suffix(&db, "-wal"),
            append_suffix(&db, "-shm"),
            append_suffix(&db, "-journal"),
        ] {
            assert!(sidecar_path(&db, Some(overlap)).is_err());
        }
        let separate = tmp.path().join("settings.sqlite3");
        assert_eq!(sidecar_path(&db, Some(separate.clone())).unwrap(), separate);
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_and_hardlink_to_archive() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path().join("archive.db");
        fs::File::create(&archive).unwrap();
        let alias = tmp.path().join("alias.db");
        std::os::unix::fs::symlink(&archive, &alias).unwrap();
        assert!(sidecar_path(&archive, Some(alias.clone())).is_err());
        fs::remove_file(&alias).unwrap();
        fs::hard_link(&archive, &alias).unwrap();
        assert!(sidecar_path(&archive, Some(alias)).is_err());
    }
}
