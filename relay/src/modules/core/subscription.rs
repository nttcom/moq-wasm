use crate::modules::enums::ContentExists;

pub struct Subscription {
    inner: moqt::Subscription,
}

impl Subscription {
    pub(crate) fn as_moqt(&self) -> &moqt::Subscription {
        &self.inner
    }

    pub(crate) fn request_id(&self) -> u64 {
        self.inner.request_id
    }

    pub(crate) fn track_alias(&self) -> u64 {
        self.inner.track_alias
    }

    pub(crate) fn expires(&self) -> u64 {
        self.inner.expires
    }

    pub(crate) fn content_exists(&self) -> ContentExists {
        ContentExists::from(self.inner.content_exists)
    }
}

impl From<moqt::Subscription> for Subscription {
    fn from(inner: moqt::Subscription) -> Self {
        Self { inner }
    }
}
