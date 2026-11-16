//! The Task board's transition table (design 3.4), every row for every
//! actor, and the guard that with the board off the house rule is unchanged.

use super::board::{settle_board, transition, Asker, Transition};
use super::TaskError;
use crate::hub::ExtensionError;
use protocol::TaskStatus::{self, Cancelled, Done, Open, Review, Taken};

const ALL: [TaskStatus; 5] = [Open, Taken, Review, Done, Cancelled];

fn operator() -> Asker {
    Asker {
        board: true,
        operator: true,
        ..Asker::default()
    }
}
fn lead() -> Asker {
    Asker {
        board: true,
        lead: true,
        ..Asker::default()
    }
}
fn holder() -> Asker {
    Asker {
        board: true,
        mine: true,
        ..Asker::default()
    }
}
fn other() -> Asker {
    Asker {
        board: true,
        ..Asker::default()
    }
}
fn with_reason(asker: Asker) -> Asker {
    Asker {
        has_reason: true,
        ..asker
    }
}

#[test]
fn the_holder_asks_for_review_instead_of_finishing() {
    for from in [Open, Taken] {
        for asked in [Done, Review] {
            assert_eq!(
                transition(from, asked, holder()),
                Ok((Review, Transition::ReviewAsked)),
                "{from:?} -> {asked:?}"
            );
        }
    }
    assert_eq!(
        transition(Open, Taken, holder()),
        Ok((Taken, Transition::Plain))
    );
    // The holder cannot take finished work back or approve its own review.
    for (from, asked) in [
        (Review, Done),
        (Review, Taken),
        (Done, Taken),
        (Done, Review),
    ] {
        assert_eq!(
            transition(from, asked, holder()),
            Err(TaskError::Forbidden),
            "{from:?} -> {asked:?}"
        );
    }
}

#[test]
fn the_lead_and_the_operator_say_go_or_no_go_with_a_reason() {
    for reviewer in [lead(), operator()] {
        assert_eq!(
            transition(Review, Done, reviewer),
            Ok((Done, Transition::Go))
        );
        assert_eq!(
            transition(Review, Taken, reviewer),
            Err(TaskError::ReasonRequired)
        );
        assert_eq!(
            transition(Review, Taken, with_reason(reviewer)),
            Ok((Taken, Transition::NoGo))
        );
    }
    // Nobody else reviews, a reason or not.
    for asker in [other(), with_reason(other())] {
        assert_eq!(transition(Review, Done, asker), Err(TaskError::Forbidden));
        assert_eq!(transition(Review, Taken, asker), Err(TaskError::Forbidden));
    }
}

#[test]
fn the_lead_never_reviews_its_own_task_the_operator_does() {
    let own = Asker {
        mine: true,
        ..with_reason(lead())
    };
    assert_eq!(transition(Review, Done, own), Err(TaskError::Forbidden));
    assert_eq!(transition(Review, Taken, own), Err(TaskError::Forbidden));
    // The Lead holding a task asks for review like any holder.
    assert_eq!(
        transition(Taken, Done, own),
        Ok((Review, Transition::ReviewAsked))
    );
    let own_operator = Asker {
        mine: true,
        ..operator()
    };
    assert_eq!(
        transition(Review, Done, own_operator),
        Ok((Done, Transition::Go))
    );
}

#[test]
fn removing_and_reopening_stay_the_operators_veto() {
    for from in ALL {
        assert_eq!(
            transition(from, Cancelled, operator()),
            Ok((Cancelled, Transition::Plain))
        );
        assert_eq!(
            transition(from, Open, operator()),
            Ok((Open, Transition::Plain))
        );
        for asker in [lead(), holder(), other()] {
            assert_eq!(
                transition(from, Open, asker),
                Err(TaskError::Forbidden),
                "{from:?} reopened by {asker:?}"
            );
            assert_eq!(
                transition(from, Cancelled, asker),
                Err(TaskError::Forbidden),
                "{from:?} removed by {asker:?}"
            );
        }
    }
    // The one exception: the Lead removes an untaken task it declared.
    let declarer = Asker {
        lead_declared_untaken: true,
        ..lead()
    };
    assert_eq!(
        transition(Open, Cancelled, declarer),
        Ok((Cancelled, Transition::Plain))
    );
    // The Operator may finish without a review, and take finished work back.
    assert_eq!(
        transition(Taken, Done, operator()),
        Ok((Done, Transition::Plain))
    );
    assert_eq!(
        transition(Done, Taken, operator()),
        Ok((Taken, Transition::Plain))
    );
    // The Lead neither finishes a task without a review nor takes one back.
    for (from, asked) in [(Taken, Done), (Open, Done), (Done, Taken), (Taken, Review)] {
        assert_eq!(
            transition(from, asked, lead()),
            Err(TaskError::Forbidden),
            "{from:?} -> {asked:?}"
        );
    }
}

/// With the board off, the rule is the house rule of `origin/master` (the
/// holder or an operator takes and finishes; an operator cancels and
/// reopens), plus the Lead's cancel of its own untaken declaration; the
/// review state is refused.
#[test]
fn with_the_board_off_every_rule_is_the_house_rule() {
    let off = |asker: Asker| Asker {
        board: false,
        ..asker
    };
    for from in ALL {
        for (asker, takes, cancels) in [
            (off(operator()), true, true),
            (off(holder()), true, false),
            (off(lead()), false, false),
            (off(other()), false, false),
        ] {
            for asked in [Taken, Done] {
                assert_eq!(
                    transition(from, asked, asker).is_ok(),
                    takes,
                    "{from:?} -> {asked:?} by {asker:?}"
                );
                if takes {
                    assert_eq!(
                        transition(from, asked, asker),
                        Ok((asked, Transition::Plain))
                    );
                }
            }
            assert_eq!(transition(from, Cancelled, asker).is_ok(), cancels);
            assert_eq!(transition(from, Open, asker).is_ok(), asker.operator);
            assert_eq!(transition(from, Review, asker), Err(TaskError::BoardOff));
        }
    }
}

#[test]
fn the_board_needs_focus_and_a_lead_and_leaves_with_focus() {
    let set = |ids: &[&str]| ids.iter().map(|id| id.to_string()).collect::<Vec<_>>();
    assert_eq!(
        settle_board(&set(&["focus"]), set(&["focus", "board"]), true),
        Ok(set(&["focus", "board"]))
    );
    assert_eq!(
        settle_board(&set(&["focus"]), set(&["focus", "board"]), false),
        Err(ExtensionError::BoardNeedsLead)
    );
    assert_eq!(
        settle_board(&set(&["notes"]), set(&["notes", "board"]), true),
        Err(ExtensionError::BoardNeedsFocus)
    );
    // Removing Focus removes the board in the same change.
    assert_eq!(
        settle_board(&set(&["focus", "board"]), set(&["board"]), true),
        Ok(set(&[]))
    );
    // A board already on stays on when the Lead question does not arise.
    assert_eq!(
        settle_board(
            &set(&["focus", "board"]),
            set(&["focus", "board", "notes"]),
            false
        ),
        Ok(set(&["focus", "board", "notes"]))
    );
}
