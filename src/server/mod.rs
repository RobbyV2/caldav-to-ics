use axum::Router;

pub mod route_builder;

pub async fn build_router(state: crate::api::sync::AppState) -> Router {
    route_builder::register_routes(state).await
}
