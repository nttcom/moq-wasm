use moqt::wire::{AuthorizationToken, ClientSetup, SetupParameter};

pub(crate) fn client_setup(authorization_token: Vec<AuthorizationToken>) -> ClientSetup {
    ClientSetup::new(
        vec![moqt::wire::MOQ_TRANSPORT_VERSION],
        SetupParameter {
            path: None,
            max_request_id: 1,
            authorization_token,
            max_auth_token_cache_size: None,
            authority: None,
            moq_implementation: None,
        },
    )
}
