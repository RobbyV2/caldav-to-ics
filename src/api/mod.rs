use crate::api::sync::AppState;
use axum::Router;

pub mod sync;

pub fn routes(state: AppState) -> Router {
    sync::routes(state)
}
