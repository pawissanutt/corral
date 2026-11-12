//! corral-core: task model, `.corral/` file store, config, events log.
//! Design: docs/DESIGN.md sections 4 and 5.

#[cfg(test)]
extern crate self as corral_core;

pub mod accepted_proof;
pub mod agent;
mod aggregate;
pub mod artifact;
pub mod artifact_owner;
pub use artifact_owner::{ArtifactOwner, ArtifactOwnerSource};
pub mod assistant;
pub mod assistant_workflow;
pub mod attention;
pub mod attribution;
pub mod availability;
pub mod backend;
pub mod blob;
pub mod blob_ref;
pub mod capacity;
pub mod card;
pub use capacity::{CapacityLane, CapacityLimits, CapacityPool};
pub mod clock;
mod hold;
pub use hold::TaskHold;
pub mod comments;
pub mod completion;
pub mod config;
pub mod config_doc;
mod config_events;
mod config_layers;
mod config_validation;
pub mod connection;
pub mod container_facts;
mod cowork;
pub use cowork::{
    CompletionDriver, CompletionIntent, CompletionScope, Cowork, CoworkReview, ReviewWaiver,
};
pub mod dashboard;
mod dispatch_config;
mod edited;
pub mod event_reader;
pub mod extension;
pub mod file_stamp;
pub mod fs_guard;
pub mod gate_presets;
pub mod gate_tier;
pub mod harness_compat;
pub mod hints;
pub mod history_summary;
pub mod hooks;
pub mod inbox;
pub mod infra_wait;
mod json_path;
pub mod legacy;
pub mod lessons;
pub mod memory;
pub mod messages;
pub mod migrate;
pub mod model;
pub mod model_family;
pub mod obligations;
pub mod operating;
pub mod origin;
pub mod park;
pub mod pins;
pub mod plan;
pub mod plan_policy;
pub mod pm_inbox;
pub mod pricing;
pub mod profile;
pub mod profile_catalog;
mod profile_records;
pub mod profile_selection;
pub mod profile_setup;
pub mod profile_transaction;
pub mod progress;
pub mod query;
pub mod records;
pub mod recovery;
pub mod repair;
mod repair_context;
pub use repair_context::{RepairContext, RepairProjection};
mod repair_sources;
pub use repair_sources::{RepairImportReceipt, RepairSourceEnd, RepairSources};
mod repair_runtime;
pub use repair_runtime::*;
pub mod output;
pub mod resources;
pub mod retention;
pub mod review;
pub mod review_contract;
pub mod routing;
mod runtime;
mod schema;
pub mod scratch;
pub mod session;
mod session_binding;
pub mod slot_release;
pub mod sqlite;
pub mod storage_roots;
pub mod store;
pub mod task_ref;
pub mod timing;
mod ui;
mod usage;
mod verification;
pub mod work;

pub use aggregate::*;
pub use dashboard::DashboardSchedule;

pub use artifact::ArtifactTier;
pub use attention::*;
pub use attribution::{Actor, AttributionGuard};
pub use availability::*;
pub use card::*;
pub use comments::*;
pub mod annotations;
pub use annotations::{
    AnnotationAction, AnnotationAnchor, AnnotationAuthor, AnnotationCode, AnnotationEntry,
    AnnotationError, AnnotationKind, AnnotationRequest, AnnotationResult, AnnotationSide,
    AnnotationState, AnnotationSummary, AnnotationSurface, AnnotationThread, AnnotationsQuery,
    AnnotationsView, ChangeAnnotation, OpenAnnotation,
};
pub use completion::*;
pub use config::*;
pub use config_doc::ConfigDoc;
pub use config_events::ConfigSource;
pub use config_layers::{ConfigLayer, ConfigLayers, ConfigSources};
pub use container_facts::{CheckpointMeta, ContainerFacts, CostTotals, PaneRef};
pub use event_reader::{
    EventCursor, EventReader, actionable_kind, default_pm_event, format_event_line,
};
pub use gate_presets::{GatePreset, detect_gate_presets};
pub use gate_tier::*;
pub use hints::*;
pub use history_summary::{EndedRun, HistorySummary, RecoverySummary, TokenTotals};
pub use hooks::*;
pub use infra_wait::*;
pub use json_path::{JsonPath, PathProblem, PathProblems, PathSeg, Read};
pub use legacy::{parse_task, parse_task_record, render_task, render_task_record};
pub use lessons::*;
pub use messages::{Message, MessageGuard};
pub use messages::{capture_messages, report as report_message};
pub use model::*;
pub use model_family::*;
pub use obligations::*;
pub use operating::*;
pub use origin::*;
pub use park::*;
pub use pins::*;
pub use plan::*;
pub use pricing::Price;
pub use profile::*;
pub use profile_catalog::*;
pub use profile_selection::*;
pub use profile_setup::*;
pub use profile_transaction::*;
pub use progress::*;
pub use recovery::*;
pub use repair::*;
pub use resources::*;
pub use retention::parse_bytes;
pub use retention::{ArtifactRetention, RetentionAction, RetentionOverride};
pub use review::*;
pub use review_contract::*;
pub use routing::*;
pub use runtime::UPDATE_NOTICE;
pub use schema::STORE_SCHEMA_VERSION;
pub use session::*;
pub use session_binding::{
    HerdrSessionBinding, SessionClaim, SessionLease, SessionView, UnqueuedReads,
};
pub use slot_release::*;
pub use storage_roots::{StorageCfg, StorageClass, StorageEnv, StorageLayout};
pub use store::*;
pub use task_ref::*;
pub use ui::{Accent, BOARD_KEYS, UiCfg};
pub use usage::{CostBasis, Usage, UsageEntry};
pub use verification::*;
