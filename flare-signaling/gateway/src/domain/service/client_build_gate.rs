//! CONNECT 版本门禁：客户端构建低于最低构建号（已生效）或落在封禁区间时，要求升级。
//!
//! 策略来自应用的 `client_policy` 投影，按发布清单平台段（`web`、`android`、`ios`、`tauri`、
//! `electron`）取；没报构建号的连接按 0 判（预发布期不留宽容窗口）。

use flare_access::manifest::{normalize_device_platform, release_platform};
use flare_im_contracts::domain::tenant::TenantClientPolicy;

/// 为什么要求升级。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpgradeReason {
    /// 低于已生效的最低构建号
    BelowMinBuild,
    /// 落在封禁区间里
    BlockedBuild,
}

impl UpgradeReason {
    /// `handshake_rejected_total{reason}` 的取值。
    pub fn metric_label(self) -> &'static str {
        match self {
            Self::BelowMinBuild => "below_min_build",
            Self::BlockedBuild => "blocked_build",
        }
    }
}

/// 门禁的拒绝：客户端至少要升级到 `min_build`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UpgradeRequired {
    pub min_build: u64,
    pub reason: UpgradeReason,
}

/// 判定一条连接是否必须升级；放行返回 `None`。
///
/// - 平台没有策略：放行。
/// - `min_build` 为 0 不限；`min_build_effective_from_ms` 在 `now_ms` 之后时还没生效（0 = 立即生效）。
/// - 封禁区间是闭区间。命中时要求的构建号取「已生效的最低构建号」与「命中区间之后的第一个构建」
///   中较大的那个，客户端照着它升级就不会再落回封禁区间。
pub fn upgrade_verdict(
    policy: &TenantClientPolicy,
    platform: &str,
    build: u64,
    now_ms: i64,
) -> Option<UpgradeRequired> {
    let build_policy = policy.build_policies.get(platform)?;
    let min_build =
        if build_policy.min_build > 0 && build_policy.min_build_effective_from_ms <= now_ms {
            build_policy.min_build
        } else {
            0
        };
    if build < min_build {
        return Some(UpgradeRequired {
            min_build,
            reason: UpgradeReason::BelowMinBuild,
        });
    }
    let past_blocked = build_policy
        .blocked
        .iter()
        .filter(|range| range.from <= build && build <= range.to)
        .map(|range| range.to.saturating_add(1))
        .max()?;
    Some(UpgradeRequired {
        min_build: min_build.max(past_blocked),
        reason: UpgradeReason::BlockedBuild,
    })
}

/// 门禁与构建分布用的平台：CONNECT 设备平台按 `flare_access::manifest::release_platform` 映射成
/// 发布清单平台段（别名先归一，与 Social 网关同一处）。没有清单的平台（鸿蒙等）和认不出外壳的桌面
/// （`distribution` 不是 `tauri` / `electron`）不设门禁，返回 `None`。
pub fn gate_platform(device_platform: &str, distribution: Option<&str>) -> Option<&'static str> {
    release_platform(device_platform, distribution)
}

/// 构建分布统计里的平台标签：发布清单平台段；鸿蒙记 `harmonyos`，其余认不出的（包括认不出外壳的
/// 桌面）记 `other`（客户端自报的平台字符串不进指标标签）。
pub fn stats_platform(device_platform: Option<&str>, distribution: Option<&str>) -> &'static str {
    let Some(device_platform) = device_platform else {
        return "other";
    };
    gate_platform(device_platform, distribution).unwrap_or_else(|| {
        if normalize_device_platform(device_platform) == Some("harmonyos") {
            "harmonyos"
        } else {
            "other"
        }
    })
}

/// CONNECT 上报的构建号：没报、不是十进制整数都按 0。
pub fn reported_build(value: Option<&str>) -> u64 {
    value
        .map(str::trim)
        .and_then(|value| value.parse().ok())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use flare_im_contracts::domain::tenant::{ClientBuildPolicy, ClientBuildRange};
    use std::collections::BTreeMap;

    const NOW: i64 = 1_800_000_000_000;

    fn policy(platform: &str, build_policy: ClientBuildPolicy) -> TenantClientPolicy {
        TenantClientPolicy {
            tenant_id: "t1".to_string(),
            build_policies: BTreeMap::from([(platform.to_string(), build_policy)]),
            ..TenantClientPolicy::default()
        }
    }

    fn below(min_build: u64) -> Option<UpgradeRequired> {
        Some(UpgradeRequired {
            min_build,
            reason: UpgradeReason::BelowMinBuild,
        })
    }

    fn blocked(min_build: u64) -> Option<UpgradeRequired> {
        Some(UpgradeRequired {
            min_build,
            reason: UpgradeReason::BlockedBuild,
        })
    }

    #[test]
    fn judgment_table() {
        let min_only = policy(
            "android",
            ClientBuildPolicy {
                min_build: 200,
                ..ClientBuildPolicy::default()
            },
        );
        let future_min = policy(
            "android",
            ClientBuildPolicy {
                min_build: 200,
                min_build_effective_from_ms: NOW + 1,
                ..ClientBuildPolicy::default()
            },
        );
        let reached_min = policy(
            "android",
            ClientBuildPolicy {
                min_build: 200,
                min_build_effective_from_ms: NOW,
                ..ClientBuildPolicy::default()
            },
        );
        let with_blocked = policy(
            "android",
            ClientBuildPolicy {
                min_build: 100,
                min_build_effective_from_ms: 0,
                blocked: vec![
                    ClientBuildRange { from: 210, to: 212 },
                    ClientBuildRange { from: 212, to: 215 },
                ],
            },
        );
        let blocked_with_future_min = policy(
            "android",
            ClientBuildPolicy {
                min_build: 300,
                min_build_effective_from_ms: NOW + 60_000,
                blocked: vec![ClientBuildRange { from: 210, to: 212 }],
            },
        );
        let unlimited = policy("android", ClientBuildPolicy::default());

        for (name, policy, platform, build, expected) in [
            ("below min", &min_only, "android", 199, below(200)),
            ("at min", &min_only, "android", 200, None),
            ("above min", &min_only, "android", 201, None),
            (
                "missing build counts as 0",
                &min_only,
                "android",
                0,
                below(200),
            ),
            ("min not effective yet", &future_min, "android", 1, None),
            (
                "min effective right now",
                &reached_min,
                "android",
                199,
                below(200),
            ),
            ("unknown platform", &min_only, "ios", 1, None),
            ("no limits", &unlimited, "android", 0, None),
            (
                "just before blocked range",
                &with_blocked,
                "android",
                209,
                None,
            ),
            (
                "blocked range start",
                &with_blocked,
                "android",
                210,
                blocked(213),
            ),
            (
                "blocked range end",
                &with_blocked,
                "android",
                212,
                blocked(216),
            ),
            (
                "overlapping range end",
                &with_blocked,
                "android",
                215,
                blocked(216),
            ),
            (
                "just after blocked range",
                &with_blocked,
                "android",
                216,
                None,
            ),
            (
                "below min before blocked",
                &with_blocked,
                "android",
                99,
                below(100),
            ),
            (
                "blocked while min is pending",
                &blocked_with_future_min,
                "android",
                211,
                blocked(213),
            ),
        ] {
            assert_eq!(
                upgrade_verdict(policy, platform, build, NOW),
                expected,
                "{name}"
            );
        }
    }

    #[test]
    fn blocked_build_asks_for_at_least_the_effective_min_build() {
        let policy = policy(
            "web",
            ClientBuildPolicy {
                min_build: 500,
                min_build_effective_from_ms: 0,
                blocked: vec![ClientBuildRange { from: 600, to: 601 }],
            },
        );
        assert_eq!(upgrade_verdict(&policy, "web", 600, NOW), blocked(602));
        let policy = TenantClientPolicy {
            build_policies: BTreeMap::from([(
                "web".to_string(),
                ClientBuildPolicy {
                    min_build: 900,
                    min_build_effective_from_ms: 0,
                    blocked: vec![ClientBuildRange { from: 0, to: 10 }],
                },
            )]),
            ..TenantClientPolicy::default()
        };
        // 0 低于最低构建号，按最低构建号报
        assert_eq!(upgrade_verdict(&policy, "web", 0, NOW), below(900));
    }

    #[test]
    fn platforms_follow_the_shared_release_mapping() {
        assert_eq!(gate_platform("pc", Some("electron")), Some("electron"));
        assert_eq!(gate_platform("pc", Some("tauri")), Some("tauri"));
        assert_eq!(gate_platform("pc", None), None);
        assert_eq!(gate_platform("pc", Some("direct")), None);
        assert_eq!(gate_platform("desktop", Some("tauri")), Some("tauri"));
        assert_eq!(gate_platform("iphone", None), Some("ios"));
        assert_eq!(stats_platform(Some("pc"), None), "other");
        assert_eq!(stats_platform(Some("harmony"), None), "harmonyos");
        assert_eq!(gate_platform("web", None), Some("web"));
        assert_eq!(gate_platform("harmonyos", None), None);
        assert_eq!(stats_platform(Some("harmonyos"), None), "harmonyos");
        assert_eq!(stats_platform(Some("android"), Some("play")), "android");
        assert_eq!(stats_platform(Some("toaster"), None), "other");
        assert_eq!(stats_platform(None, None), "other");
    }

    #[test]
    fn reported_build_defaults_to_zero() {
        assert_eq!(reported_build(None), 0);
        assert_eq!(reported_build(Some("")), 0);
        assert_eq!(reported_build(Some("1.2.3")), 0);
        assert_eq!(reported_build(Some("-5")), 0);
        assert_eq!(reported_build(Some(" 20261104 ")), 20_261_104);
    }
}
