use super::*;
use serde_json::Value;
use sha2::{Digest, Sha256};

async fn get(base: &str, path: &str) -> reqwest::Response {
    reqwest::Client::new()
        .get(format!("{base}{path}"))
        .send()
        .await
        .unwrap()
}

#[tokio::test]
async fn skill_bundles_are_public_versioned_and_byte_consistent() {
    // CLOSED building (ROOM_TOKEN + REQUIRE_SESSIONS): the skill-download endpoint
    // must still be PUBLIC, because an outside agent installs `loca` before it
    // has any identity. Onboarding would break if this were membership-gated.
    let (port, _guard) = spawn_server_env(
        "MASTER",
        &[
            ("ROOM_TOKEN", "rt-closed".into()),
            ("REQUIRE_SESSIONS", "1".into()),
        ],
    )
    .await;
    let base = format!("http://127.0.0.1:{port}");

    // Discovery index lists both skills with their manifests.
    let idx: Value = get(&base, "/downloads/skills").await.json().await.unwrap();
    let names: Vec<&str> = idx["skills"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"loca"), "index must list loca");
    assert!(names.contains(&"loca-care"), "index must list loca-care");

    for name in ["loca", "loca-care"] {
        let man: Value = get(&base, &format!("/downloads/skills/{name}/manifest"))
            .await
            .json()
            .await
            .unwrap();
        assert_eq!(man["name"], name);
        let version = man["skill_version"].as_str().unwrap();
        assert!(
            version.chars().next().unwrap().is_ascii_digit(),
            "a real skill version"
        );
        let claimed_sha = man["bundle_sha256"].as_str().unwrap().to_string();

        // Download the archive and prove its bytes hash to EXACTLY the manifest's
        // SHA-256 — i.e. the served bytes are the packaged artifact the Desktop
        // also embeds (identical bytes/checksum on both surfaces).
        let resp = get(&base, &format!("/downloads/skills/{name}")).await;
        assert_eq!(resp.status(), 200);
        assert_eq!(resp.headers()["content-type"], "application/zip");
        let bytes = resp.bytes().await.unwrap();
        assert_eq!(&bytes[..4], b"PK\x03\x04", "{name} must be a real zip");
        let mut hasher = Sha256::new();
        hasher.update(&bytes);
        assert_eq!(
            format!("{:x}", hasher.finalize()),
            claimed_sha,
            "served bytes must match the manifest SHA-256 for {name}"
        );

        // No secret-bearing or test/cache file is listed in the manifest.
        for file in man["files"].as_array().unwrap() {
            let p = file["path"].as_str().unwrap();
            assert!(
                !p.contains(".env")
                    && !p.contains(".request")
                    && !p.contains("/tests/")
                    && !p.contains("__pycache__"),
                "no secret/test file may ship in {name}: {p}"
            );
        }
    }

    // The loca bundle must ship the onboarding files — the whole point of it.
    let loca: Value = get(&base, "/downloads/skills/loca/manifest")
        .await
        .json()
        .await
        .unwrap();
    let paths: Vec<&str> = loca["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["path"].as_str().unwrap())
        .collect();
    for expected in [
        "loca/connect.sh",
        "loca/join_request.py",
        "loca/stop_listener.py",
        "loca/SKILL.md",
    ] {
        assert!(paths.contains(&expected), "loca bundle must ship {expected}");
    }

    // Unknown and traversal-shaped names resolve by EXACT name (no filesystem
    // path is ever touched), so both are a clean 404.
    assert_eq!(get(&base, "/downloads/skills/nope").await.status(), 404);
    assert_eq!(
        get(&base, "/downloads/skills/..%2f..%2fetc%2fpasswd")
            .await
            .status(),
        404
    );

    // The public allow-list is SCOPED to downloads: a membership-gated route on
    // the same closed building still refuses an anonymous caller.
    assert_eq!(get(&base, "/rooms").await.status(), 401);

    // A sibling prefix is not a real route, so `route_layer` skips it and it is
    // a clean 404 — it never becomes a public data path. (That the ALLOW-LIST
    // itself excludes the sibling prefix is unit-tested in routes::access, where
    // the boundary is observable without a matching route.)
    assert_eq!(get(&base, "/downloads/skillsevil").await.status(), 404);
    // The exact index path and a child stay public on the closed building.
    assert_eq!(get(&base, "/downloads/skills").await.status(), 200);
}
