//! Server shell, settings, persistence baseline, and boundary limits.

use super::*;

#[tokio::test]
async fn web_shell_has_security_headers_and_http_bodies_are_bounded() {
    let (port, _guard) = spawn_server().await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();

    let shell = client.get(&base).send().await.unwrap();
    assert_eq!(shell.status(), reqwest::StatusCode::OK);
    let headers = shell.headers();
    assert_eq!(
        headers
            .get("x-content-type-options")
            .and_then(|value| value.to_str().ok()),
        Some("nosniff")
    );
    assert_eq!(
        headers
            .get("x-frame-options")
            .and_then(|value| value.to_str().ok()),
        Some("DENY")
    );
    let csp = headers
        .get("content-security-policy")
        .and_then(|value| value.to_str().ok())
        .unwrap();
    assert!(csp.contains("frame-ancestors 'none'"));
    assert!(csp.contains("connect-src 'self'"));
    assert!(csp.contains("script-src 'self'"));
    assert!(!csp.contains("script-src 'unsafe-inline'"));

    let shell_body = shell.text().await.unwrap();
    assert!(!shell_body.contains("<style>"));
    assert!(!shell_body.contains("<script>"));
    let assets = [
        ("favicon.svg", "image/svg+xml", "viewBox=\"0 0 64 64\""),
        ("styles.css", "text/css", ":root"),
        ("state.js", "text/javascript", "const state ="),
        ("socket.js", "text/javascript", "function openWs()"),
        ("people.js", "text/javascript", "function renderMembers()"),
        ("chat.js", "text/javascript", "function renderMarkdown("),
        ("admin.js", "text/javascript", "function renderSettings()"),
        ("focus.js", "text/javascript", "function renderTasks()"),
        ("memory.js", "text/javascript", "function renderNotes("),
        ("api.js", "text/javascript", "async function send()"),
        ("app.js", "text/javascript", "refreshRooms();"),
    ];
    for (name, content_type, marker) in assets {
        assert!(
            shell_body.contains(&format!("/assets/{name}")),
            "web shell must reference {name}"
        );
        let response = client
            .get(format!("{base}/assets/{name}"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::OK, "{name}");
        assert!(
            response
                .headers()
                .get("content-type")
                .and_then(|value| value.to_str().ok())
                .is_some_and(|value| value.starts_with(content_type)),
            "wrong content type for {name}"
        );
        assert_eq!(
            response
                .headers()
                .get("cache-control")
                .and_then(|value| value.to_str().ok()),
            Some("no-store")
        );
        assert_eq!(
            response
                .headers()
                .get("x-content-type-options")
                .and_then(|value| value.to_str().ok()),
            Some("nosniff")
        );
        assert!(response.text().await.unwrap().contains(marker), "{name}");
    }
    assert_eq!(
        client
            .get(format!("{base}/assets/not-present.js"))
            .send()
            .await
            .unwrap()
            .status(),
        reqwest::StatusCode::NOT_FOUND
    );

    let oversized = serde_json::json!({
        "sender": "operator",
        "sender_type": "user",
        "target": "all",
        "text": "x".repeat(70 * 1024),
    });
    let response = client
        .post(format!("{base}/rooms/general/messages"))
        .json(&oversized)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test]
async fn chat_modes_are_enforced_and_admin_bypasses() {
    let (port, _guard) = spawn_server_with("s3cret").await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();

    let post = |sender: &'static str, admin: bool| {
        let client = client.clone();
        let base = base.clone();
        async move {
            let mut req = client.post(format!("{base}/rooms/general/messages")).json(
                &serde_json::json!({ "sender": sender, "sender_type": "agent", "text": "hi" }),
            );
            if admin {
                req = req.header("x-admin-token", "s3cret");
            }
            req.send().await.unwrap().status().as_u16()
        }
    };
    let set_mode = |body: serde_json::Value, token: Option<&'static str>| {
        let client = client.clone();
        let base = base.clone();
        async move {
            let mut req = client.put(format!("{base}/rooms/general/mode")).json(&body);
            if let Some(t) = token {
                req = req.header("x-admin-token", t);
            }
            req.send().await.unwrap().status().as_u16()
        }
    };

    // Free by default.
    assert_eq!(post("web", false).await, 201);

    // Setting mode without the token is unauthorized.
    assert_eq!(
        set_mode(serde_json::json!({"mode":{"mode":"paused"}}), None).await,
        401
    );
    assert_eq!(
        set_mode(serde_json::json!({"mode":{"mode":"paused"}}), Some("wrong")).await,
        401
    );
    assert_eq!(
        set_mode(
            serde_json::json!({"mode":{"mode":"paused"}}),
            Some("s3cret")
        )
        .await,
        200
    );

    // Paused: a normal poster is blocked (403), admin bypasses (201).
    assert_eq!(post("web", false).await, 403);
    assert_eq!(post("admin", true).await, 201);

    // Restricted: only "backend" may post.
    assert_eq!(
        set_mode(
            serde_json::json!({"mode":{"mode":"restricted","allow":["backend"]}}),
            Some("s3cret")
        )
        .await,
        200
    );
    assert_eq!(post("web", false).await, 403);
    assert_eq!(post("backend", false).await, 201);

    // Round-robin [a, b]: b out of turn -> 403, a -> 201, then baton at b.
    assert_eq!(
        set_mode(
            serde_json::json!({"mode":{"mode":"roundrobin","order":["a","b"],"turn":0}}),
            Some("s3cret")
        )
        .await,
        200
    );
    assert_eq!(post("b", false).await, 403);
    assert_eq!(post("a", false).await, 201);
    assert_eq!(post("b", false).await, 201); // baton advanced to b

    // Back to free lets anyone talk again.
    assert_eq!(
        set_mode(serde_json::json!({"mode":{"mode":"free"}}), Some("s3cret")).await,
        200
    );
    assert_eq!(post("web", false).await, 201);
}

#[tokio::test]
async fn rate_limit_returns_429_and_is_admin_tunable() {
    // limit 2 / 60s so the window doesn't expire mid-test.
    let (port, _guard) = spawn_server_env(
        "tok",
        &[
            ("RATE_LIMIT", "2".into()),
            ("RATE_WINDOW_SECS", "60".into()),
        ],
    )
    .await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();

    let post =
        |sender: &'static str| {
            let (client, base) = (client.clone(), base.clone());
            async move {
                client
                .post(format!("{base}/rooms/general/messages"))
                .json(&serde_json::json!({ "sender": sender, "sender_type": "agent", "text": "x" }))
                .send().await.unwrap().status().as_u16()
            }
        };

    // Two allowed, third rejected with 429.
    assert_eq!(post("spam").await, 201);
    assert_eq!(post("spam").await, 201);
    assert_eq!(post("spam").await, 429);
    // A different sender is tracked independently.
    assert_eq!(post("other").await, 201);

    // Admin raises the limit live -> spam can post again.
    let s = client
        .put(format!("{base}/rooms/general/settings"))
        .header("x-admin-token", "tok")
        .json(&serde_json::json!({ "rate_limit": 100 }))
        .send()
        .await
        .unwrap();
    assert_eq!(s.status(), 200);
    assert_eq!(post("spam").await, 201);

    // Settings without the admin token -> 401.
    let unauth = client
        .put(format!("{base}/rooms/general/settings"))
        .json(&serde_json::json!({ "rate_limit": 1 }))
        .send()
        .await
        .unwrap();
    assert_eq!(unauth.status(), 401);
}

#[tokio::test]
async fn state_survives_restart() {
    // Reserve a fixed port + temp DB so we can stop and relaunch the same server.
    let port = {
        let l = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let p = l.local_addr().unwrap().port();
        drop(l);
        p
    };
    let db = std::env::temp_dir().join(format!("agent-room-test-{port}.db"));
    let _ = std::fs::remove_file(&db);
    let db_str = db.to_string_lossy().to_string();
    let env = |port: u16| vec![("PORT", port.to_string()), ("DB_PATH", db_str.clone())];
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();

    // Boot 1: write a message, a note, and set a restricted mode.
    {
        let (_p, _guard) = spawn_server_env("tok", &env(port)).await;
        client.post(format!("{base}/rooms/general/messages"))
            .json(&serde_json::json!({ "sender": "backend", "sender_type": "agent", "text": "persist me" }))
            .send().await.unwrap();
        client
            .post(format!("{base}/rooms/general/notes"))
            .json(
                &serde_json::json!({ "key": "api", "title": "API", "body": "v1", "by": "backend" }),
            )
            .send()
            .await
            .unwrap();
        client
            .put(format!("{base}/rooms/general/mode"))
            .header("x-admin-token", "tok")
            .json(&serde_json::json!({ "mode": { "mode": "restricted", "allow": ["backend"] } }))
            .send()
            .await
            .unwrap();
        // _guard drops here -> server killed.
    }
    // Give the OS a moment to release the port.
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Boot 2: same DB + port. Everything should be back.
    let (_p2, _guard2) = spawn_server_env("tok", &env(port)).await;

    let msgs: Vec<Value> = client
        .get(format!("{base}/rooms/general/messages"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0]["text"], "persist me");

    let note: Value = client
        .get(format!("{base}/rooms/general/notes/api"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(note["body"], "v1");

    let mode: Value = client
        .get(format!("{base}/rooms/general/mode"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(mode["mode"], "restricted");

    let _ = std::fs::remove_file(&db);
}
