//! `Outcome`: what a call tells the engine.

/// What a call tells the engine. v3's `TaskOutcome`, unchanged.
///
/// Exhaustive on purpose (R48): a host matching on it handles every case, and
/// a fifth would be a new contract, not a minor addition.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Normal completion: the audit trail records status `200`, and the
    /// workflow continues.
    Ok,
    /// Completion with an HTTP-like status, recorded on the audit trail. A
    /// `4xx` is a rejection and a `5xx` a failure, handled by the task's
    /// [`on_rejection`](crate::def::TaskDef::on_rejection) and
    /// [`on_failure`](crate::def::TaskDef::on_failure).
    Status(u16),
    /// No audit entry is recorded, and the workflow continues.
    Skip,
    /// The audit trail records status `299`, and the rest of the workflow
    /// does not run. Later workflows still see the message.
    Halt,
}
