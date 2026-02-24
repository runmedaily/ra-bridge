pub mod routes;
pub mod sse;
pub mod xml_export;

use std::sync::Arc;

use axum::routing::{delete, get, post, put};
use axum::Router;

use crate::state::AppState;

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/", get(routes::index))
        .route("/api/status", get(routes::status))
        .route("/api/zones", get(routes::zones))
        .route("/api/config", get(routes::get_config))
        .route("/api/config", put(routes::put_config))
        .route("/api/pair", post(routes::start_pair))
        .route("/api/pair/status", get(sse::pair_status_stream))
        .route("/api/discover", post(routes::discover))
        .route("/api/bridge/start", post(routes::bridge_start))
        .route("/api/bridge/stop", post(routes::bridge_stop))
        .route("/api/bridge/restart", post(routes::bridge_restart))
        .route("/api/zones/{id}/level", post(routes::set_zone_level))
        .route("/api/export/xml", get(routes::export_xml))
        .route("/DbXmlInfo.xml", get(routes::export_xml))
        .route("/api/events", get(sse::zone_events_stream))
        .route("/api/logs", get(sse::log_stream))
        // Savant discovery
        .route("/api/savant/discover", post(routes::savant_discover))
        .route(
            "/api/savant/discover/status",
            get(sse::savant_discovery_status_stream),
        )
        .route("/api/savant/remove", post(routes::savant_remove))
        // Site management (dev mode)
        .route("/api/sites", get(routes::list_sites))
        .route("/api/sites", post(routes::create_site))
        .route("/api/sites/{name}", delete(routes::delete_site))
        .route("/api/sites/{name}/activate", post(routes::activate_site))
        .route("/api/sites/{name}/rename", post(routes::rename_site))
        .with_state(state)
}
