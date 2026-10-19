use crate::api::ApiClient;
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::OnceLock;

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Home,
    Role,
    Pick,
    Editor,
}

fn route(
    action: Action,
    input: &Value,
) -> Result<(reqwest::Method, String, Option<Value>), String> {
    let id = |key: &str| {
        input[key]
            .as_u64()
            .filter(|id| *id > 0 && *id < (1u64 << 53))
            .ok_or_else(|| "invalid".to_owned())
    };
    match action {
        Action::Home => Ok((reqwest::Method::GET, "/v1/community/home".into(), None)),
        Action::Role => Ok((reqwest::Method::GET, "/v1/community/role".into(), None)),
        Action::Pick => Ok((
            reqwest::Method::POST,
            "/v1/community/picks".into(),
            Some(
                json!({"trackId":id("trackId")?,"active":input["active"].as_bool().ok_or("invalid")?}),
            ),
        )),
        Action::Editor => Ok((
            reqwest::Method::POST,
            format!("/v1/admin/editors/{}", id("userId")?),
            Some(json!({"editor":input["editor"].as_bool().ok_or("invalid")?})),
        )),
    }
}

pub async fn call(client: &ApiClient, action: Action, input: Value) -> Result<Value, String> {
    let (method, path, body) = route(action, &input)?;
    let (server, token) = client
        .relay_credentials()
        .await
        .map_err(|_| "network")?
        .ok_or("sign_in")?;
    static HTTP: OnceLock<reqwest::Client> = OnceLock::new();
    let mut request = HTTP
        .get_or_init(reqwest::Client::new)
        .request(method, format!("{server}{path}"))
        .timeout(std::time::Duration::from_secs(20))
        .header("Authorization", format!("OAuth {token}"));
    if let Some(body) = body {
        request = request.json(&body);
    }
    let mut response = request.send().await.map_err(|_| "network")?;
    let status = response.status();
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| "network")? {
        if bytes.len() + chunk.len() > 512 * 1024 {
            return Err("invalid_response".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    if !status.is_success() {
        return Err(match status.as_u16() {
            401 => "sign_in",
            403 => "forbidden",
            404 => "upgrade_backend",
            400 => "invalid",
            _ => "network",
        }
        .into());
    }
    serde_json::from_slice(&bytes).map_err(|_| "invalid_response".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn editor_requests_cannot_change_admin_or_impersonate_a_contributor() {
        let (_, path, body) = route(
            Action::Pick,
            &json!({"trackId":42,"active":true,"userId":999,"admin":true}),
        )
        .unwrap();
        assert_eq!(path, "/v1/community/picks");
        assert_eq!(body, Some(json!({"trackId":42,"active":true})));
        assert!(route(Action::Editor, &json!({"userId":"../users","editor":true})).is_err());
        assert!(route(Action::Pick, &json!({"trackId":42,"active":"true"})).is_err());
    }
}
