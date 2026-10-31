//! One vocabulary for AFUI's sessions document and its runtime controls.
use std::collections::BTreeMap;

pub(super) fn words(chinese: bool) -> BTreeMap<&'static str, &'static str> {
    [
        ("back", if chinese { "返回" } else { "Back" }),
        ("size", if chinese { "卡片高度" } else { "Card size" }),
        ("renew", if chinese { "保持打开" } else { "Keep open" }),
        ("reopen", if chinese { "重新打开" } else { "Reopen" }),
        ("fold", if chinese { "折叠" } else { "Fold" }),
        ("unfold", if chinese { "展开" } else { "Unfold" }),
        ("drop", if chinese { "隐藏" } else { "Hide" }),
        (
            "end",
            if chinese {
                "结束会话"
            } else {
                "End session"
            },
        ),
        (
            "slot_heading",
            if chinese {
                "已打开的界面"
            } else {
                "Open interfaces"
            },
        ),
        (
            "empty",
            if chinese {
                "当前没有打开的界面。"
            } else {
                "Nothing is open."
            },
        ),
        (
            "open",
            if chinese {
                "在新标签页打开"
            } else {
                "Open in a new tab"
            },
        ),
        (
            "more",
            if chinese {
                "更多操作"
            } else {
                "More actions"
            },
        ),
        ("open_count", if chinese { "个界面" } else { "open" }),
        (
            "connecting",
            if chinese {
                "正在连接"
            } else {
                "connecting"
            },
        ),
        ("live", if chinese { "已连接" } else { "live" }),
        (
            "unavailable",
            if chinese {
                "连接无响应"
            } else {
                "not answering"
            },
        ),
        ("expired", if chinese { "已过期" } else { "expired" }),
        ("short", if chinese { "矮卡片" } else { "Short card" }),
        ("tall", if chinese { "高卡片" } else { "Tall card" }),
        ("full", if chinese { "全高卡片" } else { "Full card" }),
        (
            "open_page",
            if chinese {
                "在单独页面打开 {label}"
            } else {
                "Open {label} in its own page"
            },
        ),
        (
            "view_warning",
            if chinese {
                "此视图将在 {time} 后停止，请选择保持打开。"
            } else {
                "This view stops in {time} unless you keep it open."
            },
        ),
        (
            "view_expired",
            if chinese {
                "此视图已无人值守。重新打开可查看仍在运行的会话。"
            } else {
                "This view is unattended. Reopen to view the running session."
            },
        ),
        (
            "page_warning",
            if chinese {
                "此页面将在 {time} 后进入无人值守状态，请选择保持打开。"
            } else {
                "This page becomes unattended in {time} unless you keep it open."
            },
        ),
        (
            "page_unattended",
            if chinese {
                "此页面已无人值守。请逐一重新打开需要查看的会话。"
            } else {
                "This page is unattended. Reopen each session you want to view."
            },
        ),
        (
            "keeping",
            if chinese {
                "正在保持打开…"
            } else {
                "Keeping open…"
            },
        ),
        (
            "kept",
            if chinese {
                "已保持打开"
            } else {
                "Kept open"
            },
        ),
        (
            "keep_failed",
            if chinese {
                "无法保持打开，请重试。"
            } else {
                "Couldn’t keep this open. Try again."
            },
        ),
        (
            "reopening",
            if chinese {
                "正在重新打开…"
            } else {
                "Reopening…"
            },
        ),
        (
            "reopen_failed",
            if chinese {
                "无法重新打开，请重试。"
            } else {
                "Couldn’t reopen. Try again."
            },
        ),
        (
            "reopened",
            if chinese {
                "已重新打开"
            } else {
                "Reopened"
            },
        ),
        (
            "ending",
            if chinese {
                "正在结束会话…"
            } else {
                "Ending session…"
            },
        ),
        (
            "end_failed",
            if chinese {
                "无法结束会话，请重试。"
            } else {
                "Couldn’t end the session. Try again."
            },
        ),
        (
            "ended",
            if chinese {
                "已请求结束"
            } else {
                "End requested"
            },
        ),
        (
            "page_expired",
            if chinese {
                "此链接已过期，本次交付已结束。"
            } else {
                "This link expired. This delivery has ended."
            },
        ),
    ]
    .into_iter()
    .collect()
}

/// Select the first supported, non-excluded language in the preference list.
/// Unsupported preferences fall through to English.
pub(super) fn chinese(header: Option<&str>) -> bool {
    let mut preferences: Vec<_> = header
        .unwrap_or_default()
        .split(',')
        .filter_map(|part| {
            let mut pieces = part.trim().split(';');
            let language = pieces.next()?.trim().to_ascii_lowercase();
            let quality = pieces
                .find_map(|value| value.trim().strip_prefix("q="))
                .map_or(1.0, |value| value.parse::<f32>().unwrap_or(0.0));
            (quality > 0.0).then_some((language, quality))
        })
        .collect();
    preferences.sort_by(|a, b| b.1.total_cmp(&a.1));
    for (language, _) in preferences {
        if language == "zh" || language.starts_with("zh-") {
            return true;
        }
        if language == "en" || language.starts_with("en-") || language == "*" {
            return false;
        }
    }
    false
}
