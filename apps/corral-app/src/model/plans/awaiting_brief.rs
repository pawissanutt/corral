//! What a revision under review asks of the owner beside its document: what accepting
//! publishes and what the reviewers said, taken out of the document's sections so neither is
//! stated twice, and the decide bar.
use corral_protocol::detail::PlanDetail;

use super::outline::one_line;
use super::{
    AwaitingBrief, Decide, DocumentSection, PublishMember, Publishes, ReviewerBrief, SectionBody,
};
use crate::model::daemon::Feed;
use crate::model::work::when;

/// The brief of `detail`, removing the proposed members and verdicts from `sections`.
pub(super) fn brief(
    feed: &Feed,
    detail: &PlanDetail,
    sections: &mut Vec<DocumentSection>,
    decide: Decide,
) -> AwaitingBrief {
    let reviews = sections
        .iter()
        .flat_map(|section| match &section.body {
            SectionBody::Reviews(reviews) => reviews.clone(),
            _ => Vec::new(),
        })
        .collect::<Vec<_>>();
    sections.retain(|section| {
        !matches!(
            section.body,
            SectionBody::Reviews(_) | SectionBody::Proposed(_)
        )
    });
    let reviewers = reviews
        .into_iter()
        .zip(&detail.reviews)
        .map(|(resolved, review)| {
            let verdict = one_line(&review.verdict);
            let time = review
                .at
                .as_deref()
                .map(|at| when(at, feed))
                .filter(|time| !time.is_empty());
            let byline = [review.profile.clone(), time.clone()]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>();
            let label = format!(
                "{} · {verdict}{}",
                review.revision,
                if resolved.superseded { " · old" } else { "" }
            );
            let hint = format!(
                "Review for {}{}: {}{}{}.",
                review.revision,
                if resolved.superseded {
                    ", superseded by the current revision"
                } else {
                    ""
                },
                review.verdict,
                review
                    .profile
                    .as_ref()
                    .map_or_else(String::new, |profile| format!("; reviewer {profile}")),
                review
                    .note
                    .as_ref()
                    .map_or_else(String::new, |note| format!("; {note}")),
            );
            ReviewerBrief {
                verdict,
                profile: review.profile.clone(),
                revision: review.revision.clone(),
                at: review.at.clone(),
                time,
                label,
                byline: (!byline.is_empty()).then(|| byline.join(" · ")),
                hint,
                note: review.note.clone(),
                old: resolved.superseded,
                mark: resolved.mark,
            }
        })
        .collect();
    let publishes = detail.document.as_ref().map(|document| {
        let count = document.proposed.len();
        Publishes {
            sentence: format!(
                "Publishes {count} member{}",
                if count == 1 { "" } else { "s" }
            ),
            hint: "Accepting publishes these member tasks; those that need nothing can start at once.",
            members: document
                .proposed
                .iter()
                .map(|member| PublishMember {
                    name: member.name.clone(),
                    needs: member.needs.clone(),
                    hint: if member.needs.is_empty() {
                        format!("{} can start once the plan is accepted.", member.name)
                    } else {
                        format!("{} needs {}.", member.name, member.needs.join(", "))
                    },
                })
                .collect(),
        }
    });
    AwaitingBrief {
        reviewers_heading: "Reviewer",
        reviewers_hint: "Verdicts are bound to a plan revision; a newer revision supersedes them.",
        reviewers,
        publishes,
        decide,
    }
}
