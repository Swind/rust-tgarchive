use super::*;

#[tokio::test]
async fn groups_survive_reopen_and_deleted_ids_are_never_reused() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("groups.db");
    let store = SearchGroupStore::open(&path).await.unwrap();
    let created = store
        .create("  Channels  ", vec![12, -1000000000012])
        .await
        .unwrap();
    assert_eq!(created.name, "Channels");
    assert!(created.id > 0);
    store.pool.close().await;

    let store = SearchGroupStore::open(&path).await.unwrap();
    assert_eq!(store.list().await.unwrap(), [created.clone()]);
    let empty = store.update(created.id, "Empty", vec![]).await.unwrap();
    assert_eq!(empty.id, created.id);
    assert!(store.get(created.id).await.unwrap().chat_ids.is_empty());
    store.delete(created.id).await.unwrap();
    let new = store.create("Next", vec![1]).await.unwrap();
    assert!(new.id > created.id);
    assert!(matches!(
        store.get(created.id).await,
        Err(SearchGroupError::NotFound)
    ));
    assert!(matches!(
        store.delete(created.id).await,
        Err(SearchGroupError::NotFound)
    ));
    assert!(matches!(
        store.update(created.id, "Gone", vec![]).await,
        Err(SearchGroupError::NotFound)
    ));
}

#[tokio::test]
async fn independent_connections_replace_whole_groups_and_roll_back_failed_writes() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("groups.db");
    let (a, b) = tokio::join!(SearchGroupStore::open(&path), SearchGroupStore::open(&path));
    let (a, b) = (a.unwrap(), b.unwrap());
    let id = a.create("Original", vec![1]).await.unwrap().id;
    let (left, right) = tokio::join!(
        a.update(id, "Left", vec![2, 3]),
        b.update(id, "Right", vec![4])
    );
    let (left, right) = (left.unwrap(), right.unwrap());
    let saved = a.get(id).await.unwrap();
    assert!(saved == left || saved == right);

    sqlx::query("CREATE TRIGGER fail_after_write AFTER UPDATE ON search_groups BEGIN SELECT RAISE(ABORT, 'write failed'); END")
        .execute(&a.pool)
        .await
        .unwrap();
    assert!(matches!(
        b.update(id, "Not saved", vec![]).await,
        Err(SearchGroupError::Storage(_))
    ));
    assert_eq!(a.get(id).await.unwrap(), saved);
}

#[test]
fn validates_trimmed_unicode_names_and_unique_nonzero_members() {
    let max_name = "語".repeat(80);
    let members: Vec<_> = (1..=500).collect();
    assert_eq!(
        validate_group(&format!("  {max_name}  "), &members).unwrap(),
        max_name
    );
    assert!(validate_group(" A ", &[i64::MIN, i64::MAX]).is_ok());
    assert!(validate_group("A", &[]).is_ok());
    assert!(validate_group(" \t ", &[]).is_err());
    assert!(validate_group(&"語".repeat(81), &[]).is_err());
    assert!(validate_group("A", &[0]).is_err());
    assert!(validate_group("A", &[-1, -1]).is_err());
    assert!(validate_group("A", &(1..=501).collect::<Vec<_>>()).is_err());
    assert!(validate_id(0).is_err());
    assert!(validate_id(-1).is_err());
}
