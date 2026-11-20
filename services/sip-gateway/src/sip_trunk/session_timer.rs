use std::time::Duration;

use rsipstack::sip::{Header, Headers};

/// The gateway's `Min-SE`: the shortest interval RFC 4028 §4 allows.
const MIN_INTERVAL_SECS: u32 = 90;
/// The interval the gateway's INVITE asks for (RFC 4028 §4 recommends 1800 s).
const OFFERED_INTERVAL_SECS: u32 = 1800;
/// The refresher's peer sends BYE this long before the session would expire,
/// at most (RFC 4028 §10).
const BYE_MARGIN_SECS: u32 = 32;

/// Which side refreshes the session with a re-INVITE or UPDATE.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Refresher {
    Gateway,
    Trunk,
}

/// The session timer of a dialog (RFC 4028): the refresher refreshes it
/// within each interval, and the other side ends the call when it does not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SessionTimer {
    interval_secs: u32,
    pub(crate) refresher: Refresher,
}

/// What the gateway does with a session timer a trunk's INVITE asks for.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum InviteTimer {
    /// The INVITE asks for none.
    None,
    /// Shorter than `MIN_INTERVAL_SECS`: refused with 422 and `Min-SE`.
    TooShort,
    Accepted(SessionTimer),
}

/// The headers of the gateway's INVITE: it supports timers and asks for
/// `OFFERED_INTERVAL_SECS`, leaving the refresher to the trunk.
pub(crate) fn invite_headers() -> Vec<Header> {
    vec![
        Header::Supported("timer".into()),
        Header::SessionExpires(OFFERED_INTERVAL_SECS.to_string().into()),
        min_se(),
    ]
}

/// `Min-SE`, sent with the gateway's INVITE and its 422.
pub(crate) fn min_se() -> Header {
    Header::Other("Min-SE".to_string(), MIN_INTERVAL_SECS.to_string())
}

impl SessionTimer {
    /// The timer the trunk's 2xx to the gateway's INVITE settles; `None` when
    /// the 2xx has no `Session-Expires`. Without a `refresher` the gateway,
    /// the UAC, refreshes (RFC 4028 §7.2).
    pub(crate) fn from_answer(headers: &Headers) -> Option<Self> {
        let (interval_secs, refresher) = session_expires(headers)?;
        Some(Self {
            interval_secs,
            refresher: match refresher {
                Some(Role::Uas) => Refresher::Trunk,
                Some(Role::Uac) | None => Refresher::Gateway,
            },
        })
    }

    /// The timer the trunk's INVITE asks for. Without a `refresher` the
    /// gateway leaves it to the trunk, the UAC (RFC 4028 §9).
    pub(crate) fn for_invite(headers: &Headers) -> InviteTimer {
        let Some((interval_secs, refresher)) = session_expires(headers) else {
            return InviteTimer::None;
        };
        if interval_secs < MIN_INTERVAL_SECS {
            return InviteTimer::TooShort;
        }
        InviteTimer::Accepted(Self {
            interval_secs,
            refresher: match refresher {
                Some(Role::Uas) => Refresher::Gateway,
                Some(Role::Uac) | None => Refresher::Trunk,
            },
        })
    }

    /// `Session-Expires` with the refresher, named as the `uac` or `uas` of
    /// the request it travels in or answers (RFC 4028 §4):
    /// `gateway_sends_request` for the gateway's own refresh.
    pub(crate) fn session_expires(&self, gateway_sends_request: bool) -> Header {
        let refresher = if (self.refresher == Refresher::Gateway) == gateway_sends_request {
            "uac"
        } else {
            "uas"
        };
        Header::SessionExpires(format!("{};refresher={refresher}", self.interval_secs).into())
    }

    /// How long until the gateway acts: when it refreshes, half the interval
    /// (RFC 4028 §10); when the trunk does, the interval less
    /// `min(32 s, interval / 3)`, after which the gateway ends the call.
    pub(crate) fn wait(&self) -> Duration {
        let secs = match self.refresher {
            Refresher::Gateway => self.interval_secs / 2,
            Refresher::Trunk => self.interval_secs - BYE_MARGIN_SECS.min(self.interval_secs / 3),
        };
        Duration::from_secs(secs.into())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Role {
    Uac,
    Uas,
}

/// The delta-seconds and the `refresher` of `Session-Expires`
/// (`1800;refresher=uac`), or of its compact form `x`.
fn session_expires(headers: &Headers) -> Option<(u32, Option<Role>)> {
    let value = headers.iter().find_map(|header| match header {
        Header::SessionExpires(value) => Some(value.value().to_string()),
        Header::Other(name, value) if name.eq_ignore_ascii_case("x") => Some(value.clone()),
        _ => None,
    })?;
    let mut parts = value.split(';');
    let interval_secs = parts.next()?.trim().parse().ok()?;
    let refresher = parts.find_map(|param| {
        let (name, value) = param.split_once('=')?;
        if !name.trim().eq_ignore_ascii_case("refresher") {
            return None;
        }
        match value.trim().to_ascii_lowercase().as_str() {
            "uac" => Some(Role::Uac),
            "uas" => Some(Role::Uas),
            _ => None,
        }
    });
    Some((interval_secs, refresher))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(session_expires: &str) -> Headers {
        vec![Header::SessionExpires(session_expires.into())].into()
    }

    #[test]
    fn the_refresher_is_named_from_the_side_of_each_request() {
        // Act / Assert
        let timer = SessionTimer::from_answer(&headers("90;refresher=uas")).unwrap();
        assert_eq!(
            (timer.interval_secs, timer.refresher),
            (90, Refresher::Trunk)
        );
        assert_eq!(timer.session_expires(false).value(), "90;refresher=uac");
        let timer = SessionTimer::from_answer(&headers("1800")).unwrap();
        assert_eq!(timer.refresher, Refresher::Gateway);
        assert_eq!(timer.session_expires(true).value(), "1800;refresher=uac");
        assert_eq!(SessionTimer::from_answer(&Headers::default()), None);
    }

    #[test]
    fn a_trunk_s_invite_refreshes_itself_unless_it_asks_the_gateway() {
        // Act / Assert
        let InviteTimer::Accepted(timer) = SessionTimer::for_invite(&headers("600")) else {
            panic!("a 600 s timer is accepted");
        };
        assert_eq!(timer.refresher, Refresher::Trunk);
        assert_eq!(timer.session_expires(false).value(), "600;refresher=uac");
        let InviteTimer::Accepted(timer) =
            SessionTimer::for_invite(&headers("600 ; Refresher = UAS"))
        else {
            panic!("a 600 s timer is accepted");
        };
        assert_eq!(timer.refresher, Refresher::Gateway);
        assert_eq!(timer.session_expires(false).value(), "600;refresher=uas");
        assert_eq!(timer.session_expires(true).value(), "600;refresher=uac");
        assert_eq!(
            SessionTimer::for_invite(&headers("60")),
            InviteTimer::TooShort
        );
        assert_eq!(
            SessionTimer::for_invite(&Headers::default()),
            InviteTimer::None
        );
        assert_eq!(
            SessionTimer::for_invite(&headers("soon")),
            InviteTimer::None
        );
    }

    #[test]
    fn the_refresher_acts_at_half_the_interval_and_the_other_side_just_before_it_expires() {
        // Arrange
        let timer = |interval_secs, refresher| SessionTimer {
            interval_secs,
            refresher,
        };

        // Act / Assert
        assert_eq!(
            timer(90, Refresher::Gateway).wait(),
            Duration::from_secs(45)
        );
        assert_eq!(timer(90, Refresher::Trunk).wait(), Duration::from_secs(60));
        assert_eq!(
            timer(1800, Refresher::Trunk).wait(),
            Duration::from_secs(1768)
        );
    }
}
