use gstreamer::{
    glib::{self, object::Cast},
    prelude::ElementExt,
};

pub(crate) struct GStreamerInitializer;

impl GStreamerInitializer {
    pub(crate) fn initialize() -> anyhow::Result<()> {
        gstreamer::init()?;
        Ok(())
    }

    pub(crate) fn start() -> anyhow::Result<()> {
        let pipeline_str = "videotestsrc ! videoconvert ! osxvideosink";
        let pipeline = gstreamer::parse::launch(pipeline_str).unwrap();
        let pipeline = pipeline.downcast::<gstreamer::Bin>().unwrap();
        
        pipeline.set_state(gstreamer::State::Playing)?;
        Ok(())
    }
}
