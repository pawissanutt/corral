//! The pieces screens are built from, in the look of each form: marks and rails, chips, buttons,
//! styled text, task rows, page scaffolds, notices, handover bands, tables, the toast, the
//! confirmation and sheets, the arrow keys through a menu the app draws itself, the hint behind
//! a terse fact, and the facts read at a glance (counts,
//! the loop band, a task's stage rail, a session's activity). Every function takes the app first
//! and reads its form from it.

pub mod band;
pub mod button;
pub mod chip;
pub mod glance;
pub mod hint;
pub mod list;
pub mod marks;
pub mod more;
pub mod next;
pub mod notice;
pub mod overlay;
pub mod page;
pub mod page_phone;
pub mod rows;
pub mod scope;
pub mod status;
pub mod suggest;
pub mod table;
pub mod text;
pub mod walk;
