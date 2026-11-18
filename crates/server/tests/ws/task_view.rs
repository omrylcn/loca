//! One task by its number: `GET /rooms/:id/tasks/:tid`, gated like the list
//! (it is the Loca Board's route), answering 404 for a number the loca does
//! not have.

use super::lead_tasks::{answer, id_of, Loca, ROOT};
use super::loca_board::board_on;
use super::*;

async fn one(loca: &Loca, id: u64) -> (u16, Value) {
    answer(
        loca.client
            .get(format!("{}/rooms/proj/tasks/{id}", loca.base))
            .header("x-admin-token", ROOT),
    )
    .await
}

#[tokio::test]
async fn one_task_is_read_by_its_number_and_gated_like_the_list() {
    let loca = Loca::open().await;
    let ada = loca.seat("lead-ada").await;
    let bob = loca.seat("bob").await;
    board_on(&loca, "lead-ada").await;
    let (_, task) = loca
        .declare(Some(&ada), "write the upgrade note", Some("bob"))
        .await;
    let id = id_of(&task);

    let (status, read) = one(&loca, id).await;
    assert_eq!(status, 200, "{read}");
    assert_eq!(read, loca.task(id).await, "the same task the list holds");
    // A seated participant reads it too.
    let (status, read) = answer(
        bob.on(loca
            .client
            .get(format!("{}/rooms/proj/tasks/{id}", loca.base))),
    )
    .await;
    assert_eq!(status, 200, "{read}");
    assert_eq!(read["title"], "write the upgrade note");

    let (status, missing) = one(&loca, id + 40).await;
    assert_eq!(status, 404, "{missing}");
    assert_eq!(missing, "no such task");

    // With the board off it is refused like the list.
    let off = answer(
        loca.client
            .delete(format!(
                "{}/rooms/proj/extensions/board?open_tasks=keep",
                loca.base
            ))
            .header("x-admin-token", ROOT),
    )
    .await;
    assert_eq!(off.0, 200, "{}", off.1);
    let (status, refused) = one(&loca, id).await;
    assert_eq!(status, 403, "{refused}");
    assert!(
        refused
            .as_str()
            .unwrap()
            .starts_with("The Loca Board is off in this loca"),
        "{refused}"
    );
}
