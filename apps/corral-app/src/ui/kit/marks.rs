//! The state marks: the six one-stroke rail marks, the rail of four, the actor dot, the
//! changed-since-your-last-visit triangle, and the colour of each [`Hue`].
//!
//! Desktop and phone draw the mock's SVG shapes (`assets/icons/mk-*.svg`, `actor-*.svg`,
//! `delta.svg`) as tinted images, as `ui::icons` draws its icons. A terminal shows no images, so
//! there each mark is its glyph ([`Mark::glyph`]) in one Cell, its colour carrying the meaning and
//! its shape saying it again without colour.

use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::board::{Actor, Mark, Rail, rail_label};
use crate::model::hue::Hue;
use crate::state::AppState;
use crate::ui::tokens::{Token, color};

const MK_DONE: ImageAsset = asset!("icons/mk-done.svg");
const MK_ACTIVE: ImageAsset = asset!("icons/mk-active.svg");
const MK_FAIL: ImageAsset = asset!("icons/mk-fail.svg");
const MK_STALE: ImageAsset = asset!("icons/mk-stale.svg");
const MK_NEXT: ImageAsset = asset!("icons/mk-next.svg");
const MK_NA: ImageAsset = asset!("icons/mk-na.svg");
const ACTOR_SOLID: ImageAsset = asset!("icons/actor-solid.svg");
const ACTOR_HALF: ImageAsset = asset!("icons/actor-half.svg");
const ACTOR_RING: ImageAsset = asset!("icons/actor-ring.svg");
const ACTOR_DIAMOND: ImageAsset = asset!("icons/actor-diamond.svg");
const ACTOR_SQUARE: ImageAsset = asset!("icons/actor-square.svg");
const DELTA: ImageAsset = asset!("icons/delta.svg");

/// What the delta says to a screen reader (the mock's `title`).
const DELTA_LABEL: &str = "changed since your last visit";
/// The actor shapes are 9 points (10 on a phone) inside an 11-unit box, so the held diamond,
/// turned 45 degrees, keeps its corners.
const ACTOR_BOX: f32 = 11.0 / 9.0;

/// The colour of text, marks and lines in the hue: ink for [`Hue::Neutral`], else the state's
/// colour (`--you`, `--corral`, `--worker`, `--held`, `--fail`, `--ok`). Same on every form; a
/// terminal reads it as that hue's accent.
pub fn hue_ink(hue: Hue) -> AdaptiveColor {
    color(match hue {
        Hue::Neutral => Token::Ink,
        Hue::You => Token::You,
        Hue::Corral => Token::Corral,
        Hue::Worker => Token::Worker,
        Hue::Held => Token::Held,
        Hue::Fail => Token::Fail,
        Hue::Ok => Token::Ok,
    })
}

/// The quiet fill behind a chip, notice or band in the hue (`--you-bg` ...); `--surface-2` for
/// [`Hue::Neutral`]. A terminal draws it as a band where it has bands.
pub fn hue_fill(hue: Hue) -> AdaptiveColor {
    color(match hue {
        Hue::Neutral => Token::Surface2,
        Hue::You => Token::YouBg,
        Hue::Corral => Token::CorralBg,
        Hue::Worker => Token::WorkerBg,
        Hue::Held => Token::HeldBg,
        Hue::Fail => Token::FailBg,
        Hue::Ok => Token::OkBg,
    })
}

/// The mark's own colour (`.mk.done` ink-2, `.active` corral, `.fail`, `.stale` held, `.next` and
/// `.na` ink-4). In a terminal a passed mark is full ink, since ink-2 fades there and would read
/// as not yet.
fn mark_ink(form: Form, mark: Mark) -> AdaptiveColor {
    color(match mark {
        Mark::Done if form == Form::Terminal => Token::Ink,
        Mark::Done => Token::Ink2,
        Mark::Active => Token::Corral,
        Mark::Fail => Token::Fail,
        Mark::Stale => Token::Held,
        Mark::Next | Mark::Na if form == Form::Terminal => Token::Ink3,
        Mark::Next | Mark::Na => Token::Ink4,
    })
}

const fn mark_asset(mark: Mark) -> ImageAsset {
    match mark {
        Mark::Done => MK_DONE,
        Mark::Active => MK_ACTIVE,
        Mark::Fail => MK_FAIL,
        Mark::Stale => MK_STALE,
        Mark::Next => MK_NEXT,
        Mark::Na => MK_NA,
    }
}

/// `.mk` is 12 points; the phone row's 13.
fn mark_size(form: Form) -> f32 {
    match form {
        Form::Phone => 13.0,
        Form::Desktop | Form::Terminal => 12.0,
    }
}

/// The tint of a mark drawn in `hue`: the mark's own colour for [`Hue::Neutral`].
fn tint(form: Form, mark: Mark, hue: Hue) -> AdaptiveColor {
    match hue {
        Hue::Neutral => mark_ink(form, mark),
        _ => hue_ink(hue),
    }
}

/// A tinted shape, hidden from screen readers: the element around it says what it is.
fn shape(asset: ImageAsset, width: f32, height: f32, tint: AdaptiveColor) -> AnyElement {
    image(asset)
        .w(px(width))
        .h(px(height))
        .flex_shrink_0()
        .object_fit(ResizeMode::Contain)
        .tint_color(tint)
        .accessibility_elements_hidden(true)
        .into_element()
}

/// One glyph in one Cell, for the terminal.
fn glyph(content: char, ink: AdaptiveColor) -> Text {
    text(content.to_string())
        .flex_shrink_0()
        .number_of_lines(1u32)
        .text_color(ink)
}

/// One mark of a rail, step, verdict or acceptance line (`✓ ◐ ✗ ◌ ○ ·`). `hue` overrides the
/// mark's own colour; [`Hue::Neutral`] keeps it (passed ink-2, running corral, failed red, stale
/// amber, not yet and n/a faint). Stale is dashed, so it differs from running by shape too.
/// Desktop 12 pt, phone 13 pt; terminal the glyph in one Cell. Screen readers hear
/// [`Mark::label`].
pub fn mark(app: AppState, mark: Mark, hue: Hue) -> AnyElement {
    let form = app.form;
    let ink = tint(form, mark, hue);
    match form {
        Form::Terminal => glyph(mark.glyph(), ink)
            .role(AccessibilityRole::Image)
            .label(mark.label())
            .into_element(),
        Form::Desktop | Form::Phone => {
            let size = mark_size(form);
            view()
                .flex_shrink_0()
                .role(AccessibilityRole::Image)
                .label(mark.label())
                .child(shape(mark_asset(mark), size, size, ink))
                .into_element()
        }
    }
}

/// The colour of a rail's mark: a running run stage is a worker producing, so it takes the
/// worker colour (`.mk.active.w`); every other mark keeps its own.
fn rail_ink(form: Form, stage: usize, mark: Mark) -> AdaptiveColor {
    if stage == 0 && mark == Mark::Active {
        color(Token::Worker)
    } else {
        mark_ink(form, mark)
    }
}

/// Run, gate, review, land as four marks 5 pt apart (a terminal: the four glyphs a Cell apart,
/// seven Cells). The run stage running is in the worker colour, a running gate or review in the
/// corral colour. One element for screen readers, labelled with [`rail_label`]
/// ("run passed, gate running, ...").
pub fn rail(app: AppState, rail: Rail) -> AnyElement {
    let form = app.form;
    let label = rail_label(rail);
    match form {
        Form::Terminal => {
            let mut line = text("")
                .flex_shrink_0()
                .number_of_lines(1u32)
                .role(AccessibilityRole::Image)
                .label(label);
            for (stage, mark) in rail.into_iter().enumerate() {
                if stage > 0 {
                    line = line.child(" ");
                }
                line = line
                    .child(text(mark.glyph().to_string()).text_color(rail_ink(form, stage, mark)));
            }
            line.into_element()
        }
        Form::Desktop | Form::Phone => {
            let size = mark_size(form);
            view()
                .flex_row()
                .items_center()
                .flex_shrink_0()
                .gap(px(5.0))
                .role(AccessibilityRole::Image)
                .label(label)
                .children(rail.into_iter().enumerate().map(|(stage, mark)| {
                    shape(mark_asset(mark), size, size, rail_ink(form, stage, mark))
                }))
                .into_element()
        }
    }
}

/// How an actor dot is drawn: its shape, terminal glyph and colour.
#[derive(Clone, Copy)]
struct Dot {
    asset: ImageAsset,
    glyph: char,
    ink: Token,
}

const fn dot_of(hue: Hue) -> Dot {
    match hue {
        Hue::You => Dot {
            asset: ACTOR_SOLID,
            glyph: '●',
            ink: Token::You,
        },
        Hue::Corral => Dot {
            asset: ACTOR_HALF,
            glyph: '◐',
            ink: Token::Corral,
        },
        Hue::Worker => Dot {
            asset: ACTOR_HALF,
            glyph: '◐',
            ink: Token::Worker,
        },
        Hue::Held => Dot {
            asset: ACTOR_DIAMOND,
            glyph: '◆',
            ink: Token::Held,
        },
        Hue::Fail => Dot {
            asset: ACTOR_SOLID,
            glyph: '●',
            ink: Token::Fail,
        },
        Hue::Ok => Dot {
            asset: ACTOR_SOLID,
            glyph: '●',
            ink: Token::Ok,
        },
        Hue::Neutral => Dot {
            asset: ACTOR_RING,
            glyph: '○',
            ink: Token::Ink4,
        },
    }
}

/// The 9 pt dot (10 pt on a phone) in front of a row, its shape saying who acts next even
/// without colour: needs you solid, Corral and worker a half-filled ring, held a diamond,
/// [`Hue::Neutral`] (queued) a thin ring, fail and ok solid in their colour. Terminal: `●`, `◐`,
/// `◆`, `○` in one Cell. Hidden from screen readers: the row or group around it names the actor.
pub fn dot(app: AppState, hue: Hue) -> AnyElement {
    drawn(app.form, dot_of(hue))
}

/// The task's actor dot, as [`dot`] draws its hue, with the two actors no hue tells apart:
/// parked is a rounded square outline (`□` in a terminal), done is blank room of the same size.
pub fn actor(app: AppState, actor: Actor) -> AnyElement {
    match actor {
        Actor::Parked => drawn(
            app.form,
            Dot {
                asset: ACTOR_SQUARE,
                glyph: '□',
                ink: Token::Ink4,
            },
        ),
        Actor::Done => blank_dot(app.form),
        _ => dot(app, actor.hue()),
    }
}

fn dot_size(form: Form) -> f32 {
    match form {
        Form::Phone => 10.0 * ACTOR_BOX,
        Form::Desktop | Form::Terminal => 9.0 * ACTOR_BOX,
    }
}

fn drawn(form: Form, dot: Dot) -> AnyElement {
    match form {
        // A faint neutral fades to nothing next to its ground in a terminal; ink-3 still reads.
        Form::Terminal => {
            let ink = if dot.ink == Token::Ink4 {
                Token::Ink3
            } else {
                dot.ink
            };
            glyph(dot.glyph, color(ink))
                .accessibility_elements_hidden(true)
                .into_element()
        }
        Form::Desktop | Form::Phone => {
            let size = dot_size(form);
            shape(dot.asset, size, size, color(dot.ink))
        }
    }
}

fn blank_dot(form: Form) -> AnyElement {
    match form {
        Form::Terminal => glyph(' ', color(Token::Ink))
            .accessibility_elements_hidden(true)
            .into_element(),
        Form::Desktop | Form::Phone => {
            let size = dot_size(form);
            view().size(px(size)).flex_shrink_0().into_element()
        }
    }
}

/// The small upward triangle (8 by 6 pt, ink-2) on a row or line that changed since the last
/// visit. Terminal: `▲` in ink. Screen readers hear "changed since your last visit".
pub fn delta(app: AppState) -> AnyElement {
    match app.form {
        Form::Terminal => glyph('▲', color(Token::Ink))
            .role(AccessibilityRole::Image)
            .label(DELTA_LABEL)
            .into_element(),
        Form::Desktop | Form::Phone => view()
            .flex_shrink_0()
            .role(AccessibilityRole::Image)
            .label(DELTA_LABEL)
            .child(shape(DELTA, 8.0, 6.0, color(Token::Ink2)))
            .into_element(),
    }
}
