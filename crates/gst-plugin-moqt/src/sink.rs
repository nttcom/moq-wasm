use gstreamer as gst;
use gstreamer::{glib, prelude::*};

mod imp;

glib::wrapper! {
    pub struct MoqtSink(ObjectSubclass<imp::MoqtSink>) @extends gst::Element, gst::Object;
}

pub fn register(plugin: &gst::Plugin) -> Result<(), glib::BoolError> {
    gst::Element::register(
        Some(plugin),
        "moqtsink",
        gst::Rank::NONE,
        MoqtSink::static_type(),
    )
}
