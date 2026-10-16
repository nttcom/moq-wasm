use anyhow::{Result, anyhow};
use gstreamer as gst;
use mediapack::VideoSample;

pub(crate) fn video_buffer(
    sample: &VideoSample,
    duration: Option<gst::ClockTime>,
) -> Result<gst::Buffer> {
    let mut buffer = gst::Buffer::from_slice(sample.data.clone());
    {
        let buffer = buffer
            .get_mut()
            .ok_or_else(|| anyhow!("fresh buffer is shared"))?;
        buffer.set_pts(gst::ClockTime::from_useconds(sample.pts.micros()));
        buffer.set_dts(gst::ClockTime::from_useconds(sample.dts.micros()));
        buffer.set_duration(duration);
        if !sample.is_keyframe {
            buffer.set_flags(gst::BufferFlags::DELTA_UNIT);
        }
    }
    Ok(buffer)
}
