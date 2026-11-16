//! The registry token a caller is given, through the registry token port.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use crate::contract::Deployment;
use crate::contract::access::Admin;
use adapter_memory::{IssuedRecord, MemoryRegistryTokenIssuance};
use app::domain::image::ImageAction;
use app::domain::principal::{Caller, Principal};
use app::port::driven::authorisation_decision::{
    Action, AuthorisationDecision, AuthorisationError, Decision, Resource,
};
use app::port::driven::membership::{Access, Role};
use app::port::driven::registry_token_issuance::GrantedEntry;
use app::port::driving::registry_token::{
    MAX_SCOPES, RegistryToken, RegistryTokenError, RegistryTokenRequest,
};
use app::port::driving::repositories::Repositories;
use app::use_case::registry_token::{RegistryTokenAdapters, RegistryTokenService};

const SERVICE: &str = "registry.test";
const NOW: i64 = 1_700_000_000;

/// The token service over the harness's adapters and `authorisation`, signing through the
/// returned issuance, which the scenario reads the token back from.
fn service<H: Deployment>(
    harness: &H,
    authorisation: Arc<dyn AuthorisationDecision>,
) -> (RegistryTokenService, Arc<MemoryRegistryTokenIssuance>) {
    let issuance = Arc::new(MemoryRegistryTokenIssuance::default());
    let service = RegistryTokenService::new(
        RegistryTokenAdapters {
            directory: harness.directory().clone(),
            authorisation,
            issuance: issuance.clone(),
        },
        SERVICE.to_owned(),
        || NOW,
    );
    (service, issuance)
}

fn request(scopes: &[&str]) -> RegistryTokenRequest {
    RegistryTokenRequest {
        service: SERVICE.to_owned(),
        scopes: scopes.iter().map(|scope| (*scope).to_owned()).collect(),
    }
}

/// What the token `caller` is given for `scopes` carries.
async fn carried(
    service: &RegistryTokenService,
    issuance: &MemoryRegistryTokenIssuance,
    caller: &Caller,
    scopes: &[&str],
) -> IssuedRecord {
    let grant = service.issue(caller, &request(scopes)).await.unwrap();
    issuance.carried_by(&grant.token).unwrap()
}

fn entry(name: &str, actions: &[ImageAction]) -> GrantedEntry {
    GrantedEntry {
        kind: "repository".to_owned(),
        name: name.to_owned(),
        actions: actions.to_vec(),
    }
}

/// A caller is granted the image actions it holds on the repository a scope names, and an admin
/// both; a member holding neither, and a non member, are granted none.
pub async fn a_caller_is_granted_the_image_actions_it_holds<H: Deployment>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let bob = harness.caller("bob").await;
    let cat = harness.caller("cat").await;
    let dan = harness.caller("dan").await;
    let eve = harness.caller("eve").await;
    harness.repository(&ada, "acme").await;
    let admin = Admin {
        harness: &harness,
        caller: &ada,
    };
    for member in [&bob, &cat, &dan] {
        admin.add(member, Role::Member).await;
    }
    admin.grant(&bob, "", Access::ImagePull).await;
    admin.grant(&cat, "", Access::ImagePush).await;
    admin.grant(&dan, "", Access::Write).await;
    let (service, issuance) = service(&harness, harness.authorisation().clone());
    let scope = ["repository:acme/app/web:pull,push"];

    let granted = |caller| {
        let (service, issuance) = (&service, &issuance);
        async move { carried(service, issuance, caller, &scope).await.entries }
    };
    assert_eq!(
        granted(&bob).await,
        [entry("acme/app/web", &[ImageAction::Pull])]
    );
    assert_eq!(
        granted(&cat).await,
        [entry("acme/app/web", &[ImageAction::Push])]
    );
    assert_eq!(
        granted(&ada).await,
        [entry(
            "acme/app/web",
            &[ImageAction::Pull, ImageAction::Push]
        )]
    );
    // Control: the holder of `write`, and a non member, are granted nothing on the same scope.
    assert_eq!(granted(&dan).await, [entry("acme/app/web", &[])]);
    assert_eq!(granted(&eve).await, [entry("acme/app/web", &[])]);
    // Control: an action the scope does not ask for is not granted, though it is held.
    let pull_only = carried(&service, &issuance, &ada, &["repository:acme/app/web:pull"]).await;
    assert_eq!(
        pull_only.entries,
        [entry("acme/app/web", &[ImageAction::Pull])]
    );
}

/// A scope that is malformed, names an unknown repository or organisation, has another type, or
/// asks for another action is answered as a denied one is: its name echoed with no actions.
pub async fn a_scope_that_cannot_be_granted_is_answered_like_a_denied_one<H: Deployment>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let bob = harness.caller("bob").await;
    harness.repository(&ada, "acme").await;
    let admin = Admin {
        harness: &harness,
        caller: &ada,
    };
    admin.add(&bob, Role::Member).await;
    let (service, issuance) = service(&harness, harness.authorisation().clone());
    let ungrantable = [
        "repository:acme/nothing/web:pull,push",
        "repository:nowhere/app/web:pull,push",
        "repository:acme/app:pull,push",
        "repository:ACME/app/web:pull,push",
        "repository:acme/app/web:*",
        "repository:acme/app/web:delete",
        "registry:catalog:*",
        "registry:acme/app/web:pull",
    ];

    // The admin, who is granted everything it asks that exists, is granted nothing on these.
    let record = carried(&service, &issuance, &ada, &ungrantable).await;
    assert_eq!(record.entries.len(), ungrantable.len());
    assert!(record.entries.iter().all(|entry| entry.actions.is_empty()));
    // Names are echoed exactly as written.
    assert_eq!(record.entries[0].name, "acme/nothing/web");
    assert_eq!(record.entries[3].name, "ACME/app/web");
    // A scope with no `<type>:<name>:<actions>` shape is left out, not answered.
    let record = carried(
        &service,
        &issuance,
        &ada,
        &["repository", "repository:acme/app/web:pull"],
    )
    .await;
    assert_eq!(
        record.entries,
        [entry("acme/app/web", &[ImageAction::Pull])]
    );
    // Control: the unknown repository answers as a repository the caller holds nothing on does.
    let unknown = carried(
        &service,
        &issuance,
        &bob,
        &["repository:acme/nothing/web:pull"],
    )
    .await;
    let denied = carried(&service, &issuance, &bob, &["repository:acme/app/web:pull"]).await;
    assert_eq!(unknown.entries[0].actions, denied.entries[0].actions);
    assert!(denied.entries[0].actions.is_empty());
}

/// No scope is granted nothing, and the token carries the subject, the service and the time
/// bounds the port was given.
pub async fn a_request_with_no_scope_is_granted_nothing<H: Deployment>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let (service, issuance) = service(&harness, harness.authorisation().clone());

    let grant = service.issue(&ada, &request(&[])).await.unwrap();

    let record = issuance.carried_by(&grant.token).unwrap();
    assert_eq!(
        record,
        IssuedRecord {
            subject: "ada".to_owned(),
            service: SERVICE.to_owned(),
            entries: Vec::new(),
            issued_at: NOW,
            expires_at: NOW + 300,
        }
    );
    assert_eq!(grant.expires_in, 300);
    assert_eq!(grant.issued_at, "2023-11-14T22:13:20Z");
}

/// A request for another service, or with more than sixteen scopes, is refused.
pub async fn a_request_for_another_service_or_with_too_many_scopes_is_refused<H: Deployment>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let (service, _issuance) = service(&harness, harness.authorisation().clone());
    let scopes = vec!["repository:acme/app/web:pull".to_owned(); MAX_SCOPES + 1];

    let other = RegistryTokenRequest {
        service: "other".to_owned(),
        scopes: Vec::new(),
    };
    assert_eq!(
        service.issue(&ada, &other).await.err(),
        Some(RegistryTokenError::WrongService)
    );
    let many = RegistryTokenRequest {
        service: SERVICE.to_owned(),
        scopes: scopes.clone(),
    };
    assert_eq!(
        service.issue(&ada, &many).await.err(),
        Some(RegistryTokenError::TooManyScopes)
    );
    // Control: exactly sixteen scopes are served.
    let limit = RegistryTokenRequest {
        service: SERVICE.to_owned(),
        scopes: scopes.into_iter().take(MAX_SCOPES).collect(),
    };
    assert!(service.issue(&ada, &limit).await.is_ok());
}

/// A decision over `inner` that counts the calls deciding many resources and the single ones.
struct CountsCalls {
    inner: Arc<dyn AuthorisationDecision>,
    many: AtomicU32,
    single: AtomicU32,
}

#[async_trait::async_trait]
impl AuthorisationDecision for CountsCalls {
    async fn decide(
        &self,
        subject: &Principal,
        action: Action,
        resource: &Resource,
    ) -> Result<Decision, AuthorisationError> {
        self.single.fetch_add(1, Ordering::SeqCst);
        self.inner.decide(subject, action, resource).await
    }

    async fn decide_all(
        &self,
        subject: &Principal,
        action: Action,
        resources: &[Resource],
    ) -> Result<Vec<Decision>, AuthorisationError> {
        self.many.fetch_add(1, Ordering::SeqCst);
        self.inner.decide_all(subject, action, resources).await
    }
}

/// A request with many scopes asks the decision port two times at most, over all the repositories
/// its scopes name, and none where no scope names a known repository.
pub async fn a_request_asks_the_decision_port_twice_at_most<H: Deployment>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    harness.repository(&ada, "acme").await;
    harness
        .repositories()
        .create_repository(&ada, &"acme".parse().unwrap(), &"other".parse().unwrap())
        .await
        .unwrap();
    let counted = Arc::new(CountsCalls {
        inner: harness.authorisation().clone(),
        many: AtomicU32::new(0),
        single: AtomicU32::new(0),
    });
    let (service, issuance) = service(&harness, counted.clone());

    let record = carried(
        &service,
        &issuance,
        &ada,
        &[
            "repository:acme/app/a:pull",
            "repository:acme/other/b:push",
            "repository:acme/app/c:pull,push",
            "repository:acme/nothing/d:pull",
        ],
    )
    .await;

    assert_eq!(counted.many.load(Ordering::SeqCst), 2);
    assert_eq!(counted.single.load(Ordering::SeqCst), 0);
    assert_eq!(
        record.entries,
        [
            entry("acme/app/a", &[ImageAction::Pull]),
            entry("acme/other/b", &[ImageAction::Push]),
            entry("acme/app/c", &[ImageAction::Pull, ImageAction::Push]),
            entry("acme/nothing/d", &[]),
        ]
    );
    // Control: scopes naming no known repository ask nothing.
    let before = counted.many.load(Ordering::SeqCst);
    carried(
        &service,
        &issuance,
        &ada,
        &["repository:acme/nothing/d:pull"],
    )
    .await;
    assert_eq!(counted.many.load(Ordering::SeqCst), before);
}

/// An authorisation decision whose backend is down.
struct Down;

#[async_trait::async_trait]
impl AuthorisationDecision for Down {
    async fn decide(
        &self,
        _subject: &Principal,
        _action: Action,
        _resource: &Resource,
    ) -> Result<Decision, AuthorisationError> {
        Err(AuthorisationError::Backend("down".to_owned()))
    }
}

/// A failing authorisation backend fails the request with its description, and no token is
/// issued.
pub async fn a_failing_authorisation_backend_issues_no_token<H: Deployment>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    harness.repository(&ada, "acme").await;
    let (down, _issuance) = service(&harness, Arc::new(Down));
    let (working, _issuance) = service(&harness, harness.authorisation().clone());
    let scope = request(&["repository:acme/app/web:pull"]);

    assert_eq!(
        down.issue(&ada, &scope).await.err(),
        Some(RegistryTokenError::Authorisation(
            AuthorisationError::Backend("down".to_owned())
        ))
    );
    // Control: the same request over the working policy is served.
    assert!(working.issue(&ada, &scope).await.is_ok());
}
