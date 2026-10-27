use async_trait::async_trait;
use moqt::{RequestErrorCode, namespace::NamespaceInterest};

use crate::modules::{
    auth::verified_token::{ANONYMOUS_APP_ID, VerifiedToken},
    session::{
        handler::subscribe_namespace::SubscribeNamespaceHandler,
        moqt_session_event::MoqtSessionEvent,
    },
};

// An anonymous session may only reach `anon/...`, so its SUBSCRIBE_NAMESPACE
// for the root is served as one for `anon`: clients that subscribe to the root
// unconditionally (moq-rs does on every session) still discover anonymous
// broadcasts instead of being refused.
fn is_anonymous_root(token: Option<&VerifiedToken>, track_namespace_prefix: &str) -> bool {
    track_namespace_prefix.is_empty() && token.is_some_and(VerifiedToken::is_anonymous)
}

pub(crate) fn scope_anonymous_root_prefix<'a>(
    token: Option<&VerifiedToken>,
    track_namespace_prefix: &'a str,
) -> &'a str {
    if is_anonymous_root(token, track_namespace_prefix) {
        ANONYMOUS_APP_ID
    } else {
        track_namespace_prefix
    }
}

pub(crate) fn scope_anonymous_root_subscription(
    token: Option<&VerifiedToken>,
    event: MoqtSessionEvent,
) -> MoqtSessionEvent {
    match event {
        MoqtSessionEvent::SubscribeNamespace(handler)
            if is_anonymous_root(token, handler.track_namespace_prefix()) =>
        {
            MoqtSessionEvent::SubscribeNamespace(Box::new(AnonymousRootSubscribeNamespace {
                inner: handler,
                track_namespace_prefix_tuple: vec![ANONYMOUS_APP_ID.to_string()],
            }))
        }
        event => event,
    }
}

struct AnonymousRootSubscribeNamespace {
    inner: Box<dyn SubscribeNamespaceHandler>,
    track_namespace_prefix_tuple: Vec<String>,
}

#[async_trait]
impl SubscribeNamespaceHandler for AnonymousRootSubscribeNamespace {
    fn track_namespace_prefix(&self) -> &str {
        ANONYMOUS_APP_ID
    }

    fn track_namespace_prefix_tuple(&self) -> &[String] {
        &self.track_namespace_prefix_tuple
    }

    fn interest(&self) -> NamespaceInterest {
        self.inner.interest()
    }

    fn forward(&self) -> bool {
        self.inner.forward()
    }

    async fn ok(&self) -> Result<(), moqt::TransportSendError> {
        self.inner.ok().await
    }

    async fn error(
        &self,
        code: RequestErrorCode,
        reason_phrase: String,
    ) -> Result<(), moqt::TransportSendError> {
        self.inner.error(code, reason_phrase).await
    }
}

#[cfg(test)]
mod tests {
    use moqt::namespace::NamespaceInterest;

    use super::{scope_anonymous_root_prefix, scope_anonymous_root_subscription};
    use crate::modules::{
        auth::{test_support::app_token, verified_token::VerifiedToken},
        session::moqt_session_event::MoqtSessionEvent,
        test_support::mock_session::MockSubscribeNamespaceHandler,
    };

    fn subscribe_namespace_event(track_namespace_prefix: &str) -> MoqtSessionEvent {
        MoqtSessionEvent::SubscribeNamespace(Box::new(MockSubscribeNamespaceHandler::new(
            track_namespace_prefix,
        )))
    }

    fn requested_prefix(event: &MoqtSessionEvent) -> (&str, &[String]) {
        let MoqtSessionEvent::SubscribeNamespace(handler) = event else {
            panic!("expected SubscribeNamespace, got {event:?}");
        };
        (
            handler.track_namespace_prefix(),
            handler.track_namespace_prefix_tuple(),
        )
    }

    #[test]
    fn anonymous_root_subscription_is_scoped_to_the_anon_namespace() {
        // Arrange
        let token = VerifiedToken::anonymous();

        // Act
        let event = scope_anonymous_root_subscription(Some(&token), subscribe_namespace_event(""));

        // Assert
        assert_eq!(
            requested_prefix(&event),
            ("anon", &["anon".to_string()][..])
        );
    }

    #[test]
    fn anonymous_root_subscription_keeps_what_it_asks_for() {
        // Arrange
        let token = VerifiedToken::anonymous();
        let handler = MockSubscribeNamespaceHandler {
            interest: NamespaceInterest::Tracks,
            forward: false,
            ..MockSubscribeNamespaceHandler::new("")
        };

        // Act
        let event = scope_anonymous_root_subscription(
            Some(&token),
            MoqtSessionEvent::SubscribeNamespace(Box::new(handler)),
        );

        // Assert
        let MoqtSessionEvent::SubscribeNamespace(scoped) = event else {
            panic!("expected SubscribeNamespace, got {event:?}");
        };
        assert_eq!(scoped.interest(), NamespaceInterest::Tracks);
        assert!(!scoped.forward());
    }

    #[test]
    fn anonymous_subscription_below_the_root_is_kept() {
        // Arrange
        let token = VerifiedToken::anonymous();

        // Act
        let event =
            scope_anonymous_root_subscription(Some(&token), subscribe_namespace_event("anon/room"));

        // Assert
        assert_eq!(
            requested_prefix(&event),
            ("anon/room", &["anon".to_string(), "room".to_string()][..])
        );
    }

    #[test]
    fn app_token_root_subscription_is_kept() {
        // Arrange
        let token = app_token(Some(""), Some(""));

        // Act
        let event = scope_anonymous_root_subscription(Some(&token), subscribe_namespace_event(""));

        // Assert
        assert_eq!(requested_prefix(&event), ("", &[][..]));
    }

    #[test]
    fn anonymous_root_unsubscription_is_scoped_to_the_anon_namespace() {
        // Act / Assert
        assert_eq!(
            scope_anonymous_root_prefix(Some(&VerifiedToken::anonymous()), ""),
            "anon"
        );
    }

    #[test]
    fn root_unsubscription_without_a_token_is_kept() {
        // Act / Assert
        assert_eq!(scope_anonymous_root_prefix(None, ""), "");
    }
}
