//! The `board` extension is called "Loca Board" wherever a person reads it:
//! the catalog, the Journal line of a change, and every refusal that names
//! it. Its id stays `board`, the word stored in each loca's settings and sent
//! by every client.

use super::lead_tasks::{answer, Loca, ROOT};
use super::*;

async fn extension(loca: &Loca, method: reqwest::Method, id: &str) -> (u16, Value) {
    answer(
        loca.client
            .request(method, format!("{}/rooms/proj/extensions/{id}", loca.base))
            .header("x-admin-token", ROOT),
    )
    .await
}

fn text(value: &Value) -> &str {
    value.as_str().unwrap_or_default()
}

#[tokio::test]
async fn the_board_is_called_loca_board_and_its_id_stays_board() {
    let loca = Loca::open().await;
    let ada = loca.seat("lead-ada").await;
    let _bob = loca.seat("bob").await;

    let catalog: Value = loca
        .client
        .get(format!("{}/extensions", loca.base))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let board = catalog["catalog"]
        .as_array()
        .unwrap()
        .iter()
        .find(|spec| spec["id"] == "board")
        .expect("the board is in the catalog under its id");
    assert_eq!(board["name"], "Loca Board", "{board}");

    // Turned on without a Lead.
    let (status, refused) = extension(&loca, reqwest::Method::PUT, "board").await;
    assert_eq!(status, 409, "{refused}");
    assert!(
        text(&refused).contains("the Loca Board needs a Lead"),
        "{refused}"
    );

    // Its routes while it is off: the refusal every extension gives, naming
    // it.
    loca.name_lead("lead-ada").await;
    let (status, refused) = loca.declare(Some(&ada), "plain", Some("bob")).await;
    assert_eq!(status, 403, "{refused}");
    assert!(
        text(&refused).contains("this extension is off in this loca: board (Loca Board)"),
        "{refused}"
    );

    // On, then the Lead cleared under it.
    let (status, view) = extension(&loca, reqwest::Method::PUT, "board").await;
    assert_eq!(status, 200, "{view}");
    assert!(view["enabled"]
        .as_array()
        .unwrap()
        .iter()
        .any(|id| id == "board"));
    let (status, refused) = answer(
        loca.client
            .post(format!("{}/rooms/proj/lead", loca.base))
            .header("x-admin-token", ROOT)
            .json(&serde_json::json!({ "lead": null })),
    )
    .await;
    assert_eq!(status, 409, "{refused}");
    assert_eq!(
        text(&refused),
        "the Loca Board needs a Lead — transfer the Lead or turn the Loca Board off first"
    );

    // The Journal names it.
    let (status, _) = extension(&loca, reqwest::Method::DELETE, "board").await;
    assert_eq!(status, 200);
    let journal: Vec<Value> =
        get_json(&loca.client, format!("{}/rooms/proj/journal", loca.base)).await;
    let lines: Vec<&str> = journal.iter().map(|entry| text(&entry["text"])).collect();
    assert!(lines.contains(&"extensions: added Loca Board"), "{lines:?}");
    assert!(
        lines.contains(&"extensions: removed Loca Board"),
        "{lines:?}"
    );
}
