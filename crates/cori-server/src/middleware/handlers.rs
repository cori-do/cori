use crate::middleware::auth::VerifiedSession;
use axum::{extract::Request, http::StatusCode, Json};
use serde_json::json;

pub async fn whoami(req: Request) -> Result<Json<serde_json::Value>, StatusCode> {
    if let Some(sess) = req.extensions().get::<VerifiedSession>() {
        Ok(Json(json!({
            "agent_id": sess.agent_id,
            "user_id": sess.user_id,
            "session_id": sess.session_id,
            "expiry_unix": sess.expiry_unix,
        })))
    } else {
        Ok(Json(json!({ "agent_id": null, "user_id": null })))
    }
}

pub async fn example_tool_update_record(req: Request) -> Result<Json<serde_json::Value>, StatusCode> {
    let sess = req
        .extensions()
        .get::<VerifiedSession>()
        .ok_or(StatusCode::UNAUTHORIZED)?;

    Ok(Json(json!({
        "ok": true,
        "tool": "update_record",
        "agent_id": sess.agent_id,
        "user_id": sess.user_id,
    })))
}


