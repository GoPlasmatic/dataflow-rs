use crate::engine::message::{Change, Message};
use datalogic_rs::{arena::DataArena, DataValue};

pub trait TaskFunctionHandler {
    fn execute<'a>(
        &self,
        message: &mut Message<'a>,
        input: &DataValue,
        arena: &'a DataArena,
    ) -> Result<Vec<Change<'a>>, String>;
}
