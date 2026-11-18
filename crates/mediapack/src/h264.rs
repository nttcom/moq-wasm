pub mod annexb;
pub mod avcc;
pub mod nal;
pub mod parameter_sets;

pub use avcc::{AvcDecoderConfigurationRecord, avcc_to_annexb};
pub use nal::{NalUnitType, SequenceParameterSet};
pub use parameter_sets::ParameterSetTracker;

pub type ElementaryStreamDemuxer =
    crate::elementary_stream::ElementaryStreamDemuxer<AvcDecoderConfigurationRecord>;
