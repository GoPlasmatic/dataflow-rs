//! Compiles the Rust examples in `docs/src` (the mdBook user guide) as
//! doctests.
//!
//! # Why this crate exists
//!
//! `mdbook test` cannot do this job. It only passes `-L` to rustdoc, and an
//! edition-2018+ `use dataflow_rs::…` needs the crate in the extern prelude,
//! which requires `--extern`. mdBook has no flag for that, so the book's
//! examples had never been compiled against the crate.
//!
//! Routing them through `#[doc = include_str!(…)]` instead hands the job to
//! Cargo, which wires up `--extern` for every dependency automatically. It is
//! the same mechanism `ReadmeDoctests` in `dataflow-rs` uses for `README.md`.
//!
//! # Why it is a separate crate
//!
//! `docs/` is not in the root crate's `include` list, so it is absent from the
//! published `.crate` archive. An `include_str!("../docs/…")` inside
//! `dataflow-rs` itself would reference a file that does not exist for anyone
//! who downloads the crate. This member is `publish = false`, so it never
//! ships and the paths always resolve.
//!
//! # Completeness
//!
//! The page list below is hand-maintained; `tests/coverage.rs` walks
//! `docs/src` and fails if a page is neither listed here nor in its explicit
//! skip list, so a new page cannot silently ship unverified examples.
//!
//! # Conventions
//!
//! Every ```` ```rust ```` block in the listed pages is compiled. Blocks that
//! are illustrative fragments rather than runnable code — API signature
//! listings, snippets that assume an `engine` or `message` binding from
//! surrounding prose — are tagged ```` ```rust,ignore ```` in the Markdown and
//! skipped. Prefer fixing a block over tagging it: an `ignore` is an
//! unverified claim in user-facing docs.

#![cfg(doctest)]

/// Macro to reduce the per-page boilerplate to one line.
macro_rules! doc_pages {
    ($($name:ident => $path:literal),* $(,)?) => {
        $(
            #[doc = include_str!($path)]
            pub struct $name;
        )*
    };
}

doc_pages! {
    Introduction => "../../docs/src/introduction.md",

    GettingStartedInstallation => "../../docs/src/getting-started/installation.md",
    GettingStartedQuickStart => "../../docs/src/getting-started/quick-start.md",
    GettingStartedBasicConcepts => "../../docs/src/getting-started/basic-concepts.md",

    CoreConceptsOverview => "../../docs/src/core-concepts/overview.md",
    CoreConceptsEngine => "../../docs/src/core-concepts/engine.md",
    CoreConceptsWorkflow => "../../docs/src/core-concepts/workflow.md",
    CoreConceptsTask => "../../docs/src/core-concepts/task.md",
    CoreConceptsMessage => "../../docs/src/core-concepts/message.md",
    CoreConceptsErrorHandling => "../../docs/src/core-concepts/error-handling.md",

    BuiltInsOverview => "../../docs/src/built-in-functions/overview.md",
    BuiltInsParse => "../../docs/src/built-in-functions/parse.md",
    BuiltInsMap => "../../docs/src/built-in-functions/map.md",
    BuiltInsValidation => "../../docs/src/built-in-functions/validation.md",
    BuiltInsFilter => "../../docs/src/built-in-functions/filter.md",
    BuiltInsLog => "../../docs/src/built-in-functions/log.md",
    BuiltInsPublish => "../../docs/src/built-in-functions/publish.md",
    BuiltInsIntegrations => "../../docs/src/built-in-functions/integrations.md",

    AdvancedCustomFunctions => "../../docs/src/advanced/custom-functions.md",
    AdvancedJsonLogic => "../../docs/src/advanced/jsonlogic.md",
    AdvancedAuditTrails => "../../docs/src/advanced/audit-trails.md",
    AdvancedPerformance => "../../docs/src/advanced/performance.md",

    ApiReference => "../../docs/src/api/reference.md",

    WasmOverview => "../../docs/src/wasm/overview.md",
    UiOverview => "../../docs/src/ui/overview.md",
}
