use crate::utils::{
    dirs,
    help::{self, get_clash_external_port},
};
use anyhow::Result;
use log::warn;
use serde::{Deserialize, Serialize};
use serde_yaml::{Mapping, Value};
use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    str::FromStr,
};
use tracing_attributes::instrument;

use super::Config;

#[derive(Default, Debug, Clone)]
pub struct IClashTemp(pub Mapping);

impl IClashTemp {
    pub fn new() -> Self {
        match dirs::clash_path().and_then(|path| help::read_merge_mapping(&path)) {
            Ok(map) => Self(Self::guard(map)),
            Err(err) => {
                log::error!(target: "app", "{err}");
                Self::template()
            }
        }
    }

    pub fn template() -> Self {
        let mut map = Mapping::new();

        map.insert("mixed-port".into(), 7890.into());
        map.insert("log-level".into(), "info".into());
        map.insert("allow-lan".into(), false.into());
        map.insert("mode".into(), "rule".into());
        #[cfg(debug_assertions)]
        map.insert("external-controller".into(), "127.0.0.1:9872".into());
        #[cfg(not(debug_assertions))]
        map.insert("external-controller".into(), "127.0.0.1:17650".into());
        map.insert(
            "secret".into(),
            uuid::Uuid::new_v4().to_string().to_lowercase().into(), // generate a uuid v4 as default secret to secure the communication between clash and the client
        );
        #[cfg(feature = "default-meta")]
        map.insert("unified-delay".into(), true.into());
        #[cfg(feature = "default-meta")]
        map.insert("tcp-concurrent".into(), true.into());
        map.insert("ipv6".into(), false.into());

        Self(map)
    }

    fn guard(mut config: Mapping) -> Mapping {
        let port = Self::guard_mixed_port(&config);
        let ctrl = Self::guard_server_ctrl(&config);

        config.insert("mixed-port".into(), port.into());
        config.insert("external-controller".into(), ctrl.into());
        config
    }

    pub fn patch_config(&mut self, patch: Mapping) {
        for (key, value) in patch.into_iter() {
            self.0.insert(key, value);
        }
    }

    pub fn save_config(&self) -> Result<()> {
        help::save_yaml(
            &dirs::clash_path()?,
            &self.0,
            Some("# Generated by Clash Nyanpasu"),
        )
    }

    pub fn get_mixed_port(&self) -> u16 {
        Self::guard_mixed_port(&self.0)
    }

    pub fn get_client_info(&self) -> ClashInfo {
        let config = &self.0;

        ClashInfo {
            port: Self::guard_mixed_port(config),
            server: Self::guard_client_ctrl(config),
            secret: config.get("secret").and_then(|value| match value {
                Value::String(val_str) => Some(val_str.clone()),
                Value::Bool(val_bool) => Some(val_bool.to_string()),
                Value::Number(val_num) => Some(val_num.to_string()),
                _ => None,
            }),
        }
    }

    #[allow(dead_code)]
    pub fn get_external_controller_port(&self) -> u16 {
        let server = self.get_client_info().server;
        let port = server.split(':').last().unwrap_or("9090");
        port.parse().unwrap_or(9090)
    }

    #[instrument]
    pub fn prepare_external_controller_port(&mut self) -> Result<()> {
        let strategy = Config::verge()
            .latest()
            .get_external_controller_port_strategy();
        let server = self.get_client_info().server;
        let (server_ip, server_port) = server.split_once(':').unwrap_or(("127.0.0.1", "9090"));
        let server_port = server_port.parse::<u16>().unwrap_or(9090);
        let port = get_clash_external_port(&strategy, server_port)?;
        if port != server_port {
            let new_server = format!("{}:{}", server_ip, port);
            warn!(
                "The external controller port has been changed to {}",
                new_server
            );
            let mut map = Mapping::new();
            map.insert("external-controller".into(), new_server.into());
            self.patch_config(map);
        }
        Ok(())
    }

    pub fn guard_mixed_port(config: &Mapping) -> u16 {
        let mut port = config
            .get("mixed-port")
            .and_then(|value| match value {
                Value::String(val_str) => val_str.parse().ok(),
                Value::Number(val_num) => val_num.as_u64().map(|u| u as u16),
                _ => None,
            })
            .unwrap_or(7890);
        if port == 0 {
            port = 7890;
        }
        port
    }

    pub fn guard_server_ctrl(config: &Mapping) -> String {
        config
            .get("external-controller")
            .and_then(|value| match value.as_str() {
                Some(val_str) => {
                    let val_str = val_str.trim();

                    let val = match val_str.starts_with(':') {
                        true => format!("127.0.0.1{val_str}"),
                        false => val_str.to_owned(),
                    };

                    SocketAddr::from_str(val.as_str())
                        .ok()
                        .map(|s| s.to_string())
                }
                None => None,
            })
            .unwrap_or("127.0.0.1:9090".into())
    }

    pub fn guard_client_ctrl(config: &Mapping) -> String {
        let value = Self::guard_server_ctrl(config);
        match SocketAddr::from_str(value.as_str()) {
            Ok(mut socket) => {
                if socket.ip().is_unspecified() {
                    socket.set_ip(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)));
                }
                socket.to_string()
            }
            Err(_) => "127.0.0.1:9090".into(),
        }
    }

    #[allow(unused)]
    pub fn get_tun_device_ip(&self) -> String {
        let config = &self.0;

        let ip = config
            .get("dns")
            .and_then(|value| match value {
                Value::Mapping(val_map) => Some(val_map.get("fake-ip-range").and_then(
                    |fake_ip_range| match fake_ip_range {
                        Value::String(ip_range_val) => Some(ip_range_val.replace("1/16", "2")),
                        _ => None,
                    },
                )),
                _ => None,
            })
            // 默认IP
            .unwrap_or(Some("198.18.0.2".to_string()));

        ip.unwrap()
    }
}

#[derive(Default, Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct ClashInfo {
    /// clash core port
    pub port: u16,
    /// same as `external-controller`
    pub server: String,
    /// clash secret
    pub secret: Option<String>,
}

#[test]
fn test_clash_info() {
    fn get_case<T: Into<Value>, D: Into<Value>>(mp: T, ec: D) -> ClashInfo {
        let mut map = Mapping::new();
        map.insert("mixed-port".into(), mp.into());
        map.insert("external-controller".into(), ec.into());

        IClashTemp(IClashTemp::guard(map)).get_client_info()
    }

    fn get_result<S: Into<String>>(port: u16, server: S) -> ClashInfo {
        ClashInfo {
            port,
            server: server.into(),
            secret: None,
        }
    }

    assert_eq!(
        IClashTemp(IClashTemp::guard(Mapping::new())).get_client_info(),
        get_result(7890, "127.0.0.1:9090")
    );

    assert_eq!(get_case("", ""), get_result(7890, "127.0.0.1:9090"));

    assert_eq!(get_case(65537, ""), get_result(1, "127.0.0.1:9090"));

    assert_eq!(
        get_case(8888, "127.0.0.1:8888"),
        get_result(8888, "127.0.0.1:8888")
    );

    assert_eq!(
        get_case(8888, "   :98888 "),
        get_result(8888, "127.0.0.1:9090")
    );

    assert_eq!(
        get_case(8888, "0.0.0.0:8080  "),
        get_result(8888, "127.0.0.1:8080")
    );

    assert_eq!(
        get_case(8888, "0.0.0.0:8080"),
        get_result(8888, "127.0.0.1:8080")
    );

    assert_eq!(
        get_case(8888, "[::]:8080"),
        get_result(8888, "127.0.0.1:8080")
    );

    assert_eq!(
        get_case(8888, "192.168.1.1:8080"),
        get_result(8888, "192.168.1.1:8080")
    );

    assert_eq!(
        get_case(8888, "192.168.1.1:80800"),
        get_result(8888, "127.0.0.1:9090")
    );
}

#[derive(Default, Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct IClash {
    pub mixed_port: Option<u16>,
    pub allow_lan: Option<bool>,
    pub log_level: Option<String>,
    pub ipv6: Option<bool>,
    pub mode: Option<String>,
    pub external_controller: Option<String>,
    pub secret: Option<String>,
    pub dns: Option<IClashDNS>,
    pub tun: Option<IClashTUN>,
    pub interface_name: Option<String>,
}

#[derive(Default, Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct IClashTUN {
    pub enable: Option<bool>,
    pub stack: Option<String>,
    pub auto_route: Option<bool>,
    pub auto_detect_interface: Option<bool>,
    pub dns_hijack: Option<Vec<String>>,
}

#[derive(Default, Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct IClashDNS {
    pub enable: Option<bool>,
    pub listen: Option<String>,
    pub default_nameserver: Option<Vec<String>>,
    pub enhanced_mode: Option<String>,
    pub fake_ip_range: Option<String>,
    pub use_hosts: Option<bool>,
    pub fake_ip_filter: Option<Vec<String>>,
    pub nameserver: Option<Vec<String>>,
    pub fallback: Option<Vec<String>>,
    pub fallback_filter: Option<IClashFallbackFilter>,
    pub nameserver_policy: Option<Vec<String>>,
}

#[derive(Default, Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct IClashFallbackFilter {
    pub geoip: Option<bool>,
    pub geoip_code: Option<String>,
    pub ipcidr: Option<Vec<String>>,
    pub domain: Option<Vec<String>>,
}