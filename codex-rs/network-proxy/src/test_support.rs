use std::collections::HashMap;
use std::net::IpAddr;
use std::net::Ipv4Addr;

/// DNS answers for policy fixtures, independent of the developer's DNS or VPN.
pub(crate) fn public_dns_answers() -> HashMap<String, IpAddr> {
    [
        "example.com",
        "openai.com",
        "api.openai.com",
        "github.com",
        "api.github.com",
    ]
    .into_iter()
    .map(|host| (host.to_string(), IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8))))
    .collect()
}

#[tokio::test]
async fn allowlisted_hostname_resolving_to_private_address_remains_blocked() {
    let mut config = crate::NetworkProxyConfig::default();
    config.set_allowed_domains(vec!["example.com".to_string()]);
    let mut state = crate::runtime::network_proxy_state_for_policy(config);
    state
        .dns_answers
        .insert("example.com".to_string(), IpAddr::V4(Ipv4Addr::LOCALHOST));

    pretty_assertions::assert_eq!(
        state
            .host_blocked("example.com", /*port*/ 80)
            .await
            .unwrap(),
        crate::runtime::HostBlockDecision::Blocked(
            crate::runtime::HostBlockReason::NotAllowedLocal
        )
    );
}
