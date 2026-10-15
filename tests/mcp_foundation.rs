#![deny(warnings)]
#![deny(clippy::all)]

use rust_dos::control::{ControlError, ObservationService, UnavailableObservations};

#[test]
fn observation_boundary_contains_no_transport_types() {
    let source = include_str!("../src/debug/control.rs");
    for namespace in ["rmcp::", "axum::", "http::", "serde_json::", "tokio::"] {
        assert!(
            !source.contains(namespace),
            "transport leaked into observation declarations: {namespace}"
        );
    }
    let service = include_str!("../src/debug/control/service.rs");
    for namespace in ["rmcp::", "axum::", "http::", "reqwest::", "ureq::"] {
        assert!(
            !service.contains(namespace),
            "transport leaked into actor service: {namespace}"
        );
    }
}

#[tokio::test]
async fn null_observations_never_invent_status_or_pixels() {
    let service = UnavailableObservations;
    assert_eq!(service.status().await, Err(ControlError::Unavailable));
    assert_eq!(service.capture().await, Err(ControlError::Unavailable));
    assert_eq!(
        ControlError::Unavailable.to_string(),
        "observation unavailable"
    );
}
