use crate::modules::relay::types::SubgroupKey;

#[derive(Clone, Copy, Debug)]
pub(crate) enum TrackEvent {
    SubgroupOpened(SubgroupKey),
}
