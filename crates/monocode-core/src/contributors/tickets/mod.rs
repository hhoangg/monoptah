//! Pure Ticket Contributors model: levels and scope rules, the member board,
//! saved preferences, the HTML report and the view-state helpers. Port of
//! `src/features/ticket-contributors/` (`model/` and the pure parts of
//! `hooks/useTicketBoard.ts`). The ticket type is the backend `TicketRecord`.

pub mod board;
pub mod board_state;
pub mod prefs;
pub mod report;
#[allow(clippy::module_inception)]
pub mod tickets;

#[cfg(test)]
pub mod test_fixtures;
