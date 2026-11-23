//! The Helm chart (`deploy/helm/codehub`) rendered by `helm template` for its `ci/garam.yaml`
//! values, and what the render must hold: where each public path goes, which pods mount the
//! AuthZEN key, and what the chart refuses to render. `make helm-check` runs them.

use std::process::Command;

use serde::Deserialize;
use serde_norway::Value;

/// The chart's directory.
#[cfg(test)]
const CHART: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../deploy/helm/codehub");

/// The values the chart is rendered with, as garam's cluster sets them.
#[cfg(test)]
const GARAM: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../deploy/helm/codehub/ci/garam.yaml"
);

/// The text `helm template codehub` prints for `GARAM` and then `extra` arguments, or what it
/// printed to stderr where it failed.
#[cfg(test)]
fn helm(extra: &[&str]) -> Result<String, String> {
    let output = Command::new("helm")
        .args(["template", "codehub", CHART, "-f", GARAM])
        .args(extra)
        .output()
        .unwrap();
    if output.status.success() {
        Ok(String::from_utf8(output.stdout).unwrap())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).into_owned())
    }
}

/// The objects of a render.
#[cfg(test)]
fn render(extra: &[&str]) -> Vec<Value> {
    let text = helm(extra).unwrap();
    serde_norway::Deserializer::from_str(&text)
        .map(|document| Value::deserialize(document).unwrap())
        .filter(|object| !object.is_null())
        .collect()
}

/// The object of `kind` named `name`.
#[cfg(test)]
fn object<'a>(objects: &'a [Value], kind: &str, name: &str) -> &'a Value {
    objects
        .iter()
        .find(|o| o["kind"] == kind && o["metadata"]["name"] == name)
        .unwrap_or_else(|| panic!("no {kind} named {name}"))
}

/// The pod spec of the Deployment or Job `name`.
#[cfg(test)]
fn pod<'a>(objects: &'a [Value], kind: &str, name: &str) -> &'a Value {
    &object(objects, kind, name)["spec"]["template"]["spec"]
}

/// The `(path prefix, backend Service, port)` of each rule of the HTTPRoute `name`.
#[cfg(test)]
fn backends(objects: &[Value], name: &str) -> Vec<(String, String, u64)> {
    let rules = object(objects, "HTTPRoute", name)["spec"]["rules"]
        .as_sequence()
        .unwrap();
    rules
        .iter()
        .map(|rule| {
            let path = &rule["matches"][0]["path"];
            assert_eq!(path["type"], "PathPrefix");
            let backend = &rule["backendRefs"][0];
            (
                path["value"].as_str().unwrap().to_owned(),
                backend["name"].as_str().unwrap().to_owned(),
                backend["port"].as_u64().unwrap(),
            )
        })
        .collect()
}

/// The port of the Service `name`.
#[cfg(test)]
fn service_port(objects: &[Value], name: &str) -> u64 {
    object(objects, "Service", name)["spec"]["ports"][0]["port"]
        .as_u64()
        .unwrap()
}

#[test]
fn routes_each_public_path_to_its_backend_on_the_port_of_its_service() {
    let objects = render(&[]);

    let api = backends(&objects, "codehub");
    assert_eq!(
        api,
        [
            (
                "/v1/runner/".to_owned(),
                "codehub-runner-api".to_owned(),
                service_port(&objects, "codehub-runner-api")
            ),
            (
                "/v1/".to_owned(),
                "codehub-api".to_owned(),
                service_port(&objects, "codehub-api")
            ),
        ]
    );
    assert_eq!(
        backends(&objects, "codehub-registry"),
        [(
            "/v2/".to_owned(),
            "codehub-registry".to_owned(),
            service_port(&objects, "codehub-registry")
        )]
    );
}

/// The path prefixes the routes of a render match, those of `Ingress` rules and of `HTTPRoute`
/// rules alike.
#[cfg(test)]
fn routed_prefixes(objects: &[Value]) -> Vec<String> {
    let mut prefixes = Vec::new();
    for object in objects {
        match object["kind"].as_str() {
            Some("HTTPRoute") => {
                for rule in object["spec"]["rules"].as_sequence().unwrap() {
                    for m in rule["matches"].as_sequence().unwrap() {
                        assert_eq!(m["path"]["type"], "PathPrefix", "{m:?}");
                        prefixes.push(m["path"]["value"].as_str().unwrap().to_owned());
                    }
                }
            }
            Some("Ingress") => {
                for rule in object["spec"]["rules"].as_sequence().unwrap() {
                    for path in rule["http"]["paths"].as_sequence().unwrap() {
                        assert_eq!(path["pathType"], "Prefix", "{path:?}");
                        prefixes.push(path["path"].as_str().unwrap().to_owned());
                    }
                }
            }
            _ => {}
        }
    }
    prefixes
}

#[test]
fn routes_nothing_that_reaches_the_unauthenticated_paths_of_the_server() {
    let ingress = [
        "--set",
        "ingress.enabled=true",
        "--set",
        "ingress.host=a.example",
    ];
    for (what, extra) in [("httpRoute", &[][..]), ("ingress", &ingress[..])] {
        let prefixes = routed_prefixes(&render(extra));
        // The control: the render has routes, and the public ones are among them.
        assert!(prefixes.iter().any(|p| p == "/v1/"), "{what}: {prefixes:?}");
        for path in ["/metrics", "/healthz", "/readyz"] {
            // A prefix of a path covers it, `/` and `/m` included.
            let covering: Vec<_> = prefixes.iter().filter(|p| path.starts_with(*p)).collect();
            assert!(covering.is_empty(), "{what} routes {path} by {covering:?}");
        }
    }
}

/// The names of the volumes mounted by the first container of `pod`, with whether each is
/// read-only and where it is mounted.
#[cfg(test)]
fn mounts(pod: &Value) -> Vec<(String, bool, String)> {
    let mounts = pod["containers"][0]["volumeMounts"].as_sequence();
    mounts
        .into_iter()
        .flatten()
        .map(|mount| {
            (
                mount["name"].as_str().unwrap().to_owned(),
                mount["readOnly"] == true,
                mount["mountPath"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

#[test]
fn mounts_the_authzen_key_read_only_where_it_is_read_and_nowhere_else() {
    let objects = render(&[]);
    let config = &object(&objects, "ConfigMap", "codehub")["data"];
    assert_eq!(
        config["CODEHUB_AUTHZEN_KEY_FILE"], "/etc/codehub/authzen/key.pem",
        "{config:?}"
    );

    for role in ["api", "runner-api", "worker"] {
        let pod = pod(&objects, "Deployment", &format!("codehub-{role}"));
        assert_eq!(
            mounts(pod)
                .into_iter()
                .filter(|(name, _, _)| name == "authzen-key")
                .collect::<Vec<_>>(),
            [(
                "authzen-key".to_owned(),
                true,
                "/etc/codehub/authzen".to_owned()
            )],
            "{role}"
        );
        let volume = pod["volumes"]
            .as_sequence()
            .unwrap()
            .iter()
            .find(|volume| volume["name"] == "authzen-key")
            .unwrap_or_else(|| panic!("{role} has no authzen-key volume"));
        assert_eq!(
            volume["secret"]["secretName"], "codehub-authzen-key",
            "{role}"
        );
    }

    let migrate = pod(&objects, "Job", "codehub-migrate");
    assert!(
        mounts(migrate)
            .iter()
            .all(|(name, _, _)| name != "authzen-key"),
        "migrate mounts the key"
    );
    assert!(
        migrate["volumes"]
            .as_sequence()
            .into_iter()
            .flatten()
            .all(|volume| volume["name"] != "authzen-key"),
        "migrate has the key volume"
    );
}

#[test]
fn sets_every_setting_the_grn_profile_with_private_key_jwt_requires() {
    let objects = render(&[]);
    let config = &object(&objects, "ConfigMap", "codehub")["data"];
    for setting in [
        "CODEHUB_AUTHZEN_URL",
        "CODEHUB_AUTHZEN_TOKEN_ENDPOINT",
        "CODEHUB_AUTHZEN_CLIENT_ID",
        "CODEHUB_AUTHZEN_ISSUER",
        "CODEHUB_AUTHZEN_KEY_ID",
        "CODEHUB_AUTHZEN_KEY_FILE",
        "CODEHUB_DIRECTORY_GARAM_URL",
    ] {
        assert!(
            config[setting]
                .as_str()
                .is_some_and(|value| !value.is_empty()),
            "{setting} is not set: {config:?}"
        );
    }
    assert!(
        config["CODEHUB_AUTHZEN_CLIENT_ID"]
            .as_str()
            .is_some_and(|id| id.contains("{organisation}")),
        "{config:?}"
    );
}

#[test]
fn refuses_to_render_a_route_without_parent_refs_or_hostnames() {
    // The control: the values render.
    assert!(helm(&[]).is_ok());
    for (set, what) in [
        (
            "httpRoute.parentRefs=[]",
            "httpRoute.parentRefs is required",
        ),
        ("httpRoute.hostnames=[]", "httpRoute.hostnames is required"),
        (
            "registry.httpRoute.hostnames=[]",
            "registry.httpRoute.hostnames is required",
        ),
    ] {
        let refusal = helm(&["--set-json", set]).unwrap_err();
        assert!(refusal.contains(what), "{set}: {refusal}");
    }
}
