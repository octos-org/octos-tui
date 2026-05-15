use octos_tui::model::{SessionRuntimeStatus, SessionStatusReadResult};
use serde_json::Value;

#[test]
fn solo_onboarding_fixture_uses_local_profile_create_without_otp() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../fixtures/m12_solo_onboarding_profile_local_create.json"
    ))
    .expect("fixture parses");
    let requests = fixture["client_requests"]
        .as_array()
        .expect("client requests");
    let forbidden = fixture["forbidden_methods"]
        .as_array()
        .expect("forbidden methods");

    for method in forbidden.iter().filter_map(Value::as_str) {
        assert!(
            !requests
                .iter()
                .any(|request| request["method"].as_str() == Some(method)),
            "solo fixture must not emit {method}"
        );
    }

    let create_result = fixture["server_results"]
        .as_array()
        .expect("server results")
        .iter()
        .find(|result| result["method"].as_str() == Some("profile/local/create"))
        .expect("profile/local/create result");
    let server_profile_id = create_result["result"]["profile_id"]
        .as_str()
        .expect("server profile_id");
    let session_open = requests
        .iter()
        .find(|request| request["method"].as_str() == Some("session/open"))
        .expect("session/open request");

    assert_eq!(
        session_open["params"]["profile_id"].as_str(),
        Some(server_profile_id)
    );
}

#[test]
fn permissions_fixture_requires_server_confirmed_dangerous_status() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../fixtures/m12_permissions_server_truth.json"
    ))
    .expect("fixture parses");
    let cases = fixture["status_cases"].as_array().expect("status cases");

    for case in cases {
        let status: SessionStatusReadResult = serde_json::from_value(serde_json::json!({
            "session_id": "local:test",
            "runtime_mode": case["status"]["runtime_mode"].clone(),
            "profile_id": case["status"]["profile_id"].clone(),
            "workspace_root": case["status"]["workspace_root"].clone(),
            "approval_policy": case["status"]["approval_policy"].clone(),
            "sandbox_mode": case["status"]["sandbox_mode"].clone(),
            "permission_profile": case["status"]["permission_profile"].clone(),
            "filesystem_scope": case["status"]["filesystem_scope"].clone(),
            "network": case["status"]["network"].clone()
        }))
        .expect("status shape");
        let status = SessionRuntimeStatus::from(status);
        let dangerous = status.sandbox_mode.as_deref() == Some("danger-full-access")
            && status.filesystem_scope.as_deref() == Some("host");

        assert_eq!(
            dangerous,
            case["expect_dangerous_display"]
                .as_bool()
                .expect("danger expectation"),
            "case {}",
            case["name"].as_str().unwrap_or("<unnamed>")
        );
    }
}
