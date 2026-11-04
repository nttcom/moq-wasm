mod log_record;
mod span_record;

pub use log_record::log_record;
pub use span_record::{SpanRecordLayer, current_traceparent};
