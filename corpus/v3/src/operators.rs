//! The custom operators a case can register, by name.

use dataflow_rs::datalogic_rs::bumpalo::Bump;
use dataflow_rs::datalogic_rs::operator::EvalContext;
use dataflow_rs::datalogic_rs::{ArenaExt, CustomOperator, DataValue};

/// `shout`: uppercases its first argument.
pub struct Shout;

impl CustomOperator for Shout {
    fn evaluate<'a>(
        &self,
        args: &[&'a DataValue<'a>],
        _ctx: &mut EvalContext<'_, 'a>,
        arena: &'a Bump,
    ) -> dataflow_rs::datalogic_rs::Result<&'a DataValue<'a>> {
        let s = args.first().and_then(|v| v.as_str()).unwrap_or_default();
        Ok(arena.string(&s.to_uppercase()))
    }
}
