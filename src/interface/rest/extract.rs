use axum::{
    extract::{
        FromRequest, FromRequestParts, Json, Path, Query,
        rejection::{JsonRejection, PathRejection, QueryRejection},
    },
    http::{Request, request::Parts},
};
use serde::de::DeserializeOwned;

use super::{ApiError, RequestId};

pub(crate) struct ApiQuery<T>(pub T);
pub(crate) struct ApiPath<T>(pub T);
pub(crate) struct ApiJson<T>(pub T);

fn request_id(parts: &Parts) -> String {
    parts
        .extensions
        .get::<RequestId>()
        .map(|id| id.0.clone())
        .unwrap_or_else(|| "unknown".to_owned())
}

impl<S, T> FromRequest<S> for ApiJson<T>
where
    S: Send + Sync,
    T: DeserializeOwned + Send,
{
    type Rejection = ApiError;

    async fn from_request(
        request: Request<axum::body::Body>,
        state: &S,
    ) -> Result<Self, Self::Rejection> {
        let id = request
            .extensions()
            .get::<RequestId>()
            .map_or_else(|| "unknown".to_owned(), |id| id.0.clone());
        Json::<T>::from_request(request, state)
            .await
            .map(|Json(value)| Self(value))
            .map_err(|_: JsonRejection| ApiError::malformed(id))
    }
}

impl<S, T> FromRequestParts<S> for ApiQuery<T>
where
    S: Send + Sync,
    T: DeserializeOwned + Send,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let id = request_id(parts);
        Query::<T>::from_request_parts(parts, state)
            .await
            .map(|Query(value)| Self(value))
            .map_err(|_: QueryRejection| ApiError::malformed(id))
    }
}

impl<S, T> FromRequestParts<S> for ApiPath<T>
where
    S: Send + Sync,
    T: DeserializeOwned + Send,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let id = request_id(parts);
        Path::<T>::from_request_parts(parts, state)
            .await
            .map(|Path(value)| Self(value))
            .map_err(|_: PathRejection| ApiError::malformed(id))
    }
}
