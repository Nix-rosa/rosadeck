//! `rosadeck-selector`: pure selection model for the display overlay.
//!
//! No IO, no UI toolkit, no Hyprland. Given identity + mode lists + an
//! optional saved profile, it builds the valid actions and the real mode
//! options per action (never two independent dropdowns that could combine
//! into a nonexistent mode). The UI only renders and returns a [`Selection`].

use display_core::{intersect_modes, DisplayIdentity, Mode, ModePolicy, OutputId};
use rosadeck_planner::{DisplayRequest, ExtendPosition, PlanKind};
use serde::{Deserialize, Serialize};

/// One selectable overlay action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SelectorAction {
    /// Full gaming session on the external output (display + desktop).
    ExternalGaming,
    /// Apply the saved profile (only offered when valid).
    UseSavedProfile,
    /// Mirror internal + external on a common mode.
    Duplicate,
    /// External on, internal off.
    ExternalOnly,
    /// Side by side.
    Extend,
    /// Do nothing.
    Cancel,
}

impl SelectorAction {
    /// Human label for the overlay.
    pub fn label(self) -> &'static str {
        match self {
            Self::ExternalGaming => "External Gaming",
            Self::UseSavedProfile => "Use saved profile",
            Self::Duplicate => "Duplicate",
            Self::ExternalOnly => "External Only",
            Self::Extend => "Extend",
            Self::Cancel => "Cancel",
        }
    }
}

/// Saved profile summary (projected from a `DisplayProfile`; no emulator data).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedProfile {
    /// Profile alias.
    pub alias: String,
    /// Saved disposition.
    pub kind: PlanKind,
    /// Saved explicit mode, if any.
    pub mode: Option<Mode>,
    /// Saved policy.
    pub policy: ModePolicy,
}

/// Inputs to build a selection context (assembled by the daemon, pure here).
#[derive(Debug, Clone)]
pub struct ContextArgs {
    /// Target external connector.
    pub connector: OutputId,
    /// EDID monitor name, if known.
    pub edid_name: Option<String>,
    /// Stable identity, if known.
    pub identity: Option<DisplayIdentity>,
    /// Currently active mode of the target, if any.
    pub current: Option<Mode>,
    /// Applicable internal modes.
    pub internal_modes: Vec<Mode>,
    /// Applicable external (target) modes.
    pub external_modes: Vec<Mode>,
    /// Matched saved profile, if any.
    pub profile: Option<SavedProfile>,
}

/// Everything the overlay needs to render (serializable for daemon↔UI).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SelectionContext {
    /// Target external connector.
    pub connector: String,
    /// Title line (EDID name or connector).
    pub title: String,
    /// Subtitle (`WxH @ Hz` of best external mode, or mode count).
    pub subtitle: String,
    /// Valid actions in presentation order.
    pub actions: Vec<SelectorAction>,
    /// Saved profile summary, when offered.
    pub profile: Option<SavedProfile>,
    /// Real mode options per action (keyed by action index in `actions`).
    pub modes_per_action: Vec<Vec<Mode>>,
}

/// User's choice from the overlay.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Selection {
    /// Chosen action.
    pub action: SelectorAction,
    /// Chosen real mode (`None` = policy decides).
    pub mode: Option<Mode>,
    /// Policy for non-manual picks.
    pub policy: ModePolicy,
}

/// Build the context: valid actions + real modes per action.
pub fn build_context(args: &ContextArgs) -> SelectionContext {
    let title = args.edid_name.clone().unwrap_or_else(|| args.connector.0.clone());
    let subtitle = match args.external_modes.iter().max_by(|a, b| a.area().cmp(&b.area()).then(a.hz.total_cmp(&b.hz))) {
        Some(m) => format!("{m}"),
        None => "no modes reported".into(),
    };
    let common = intersect_modes(&args.internal_modes, &args.external_modes);
    let mut actions = Vec::new();
    let mut modes_per_action = Vec::new();
    if !args.external_modes.is_empty() {
        // Gaming session first: full desktop migration, not just outputs.
        actions.push(SelectorAction::ExternalGaming);
        modes_per_action.push(args.external_modes.clone());
    }
    if args.profile.is_some() {
        actions.push(SelectorAction::UseSavedProfile);
        // Saved modes are validated against the target's real options.
        modes_per_action.push(args.external_modes.clone());
    }
    if !common.is_empty() {
        actions.push(SelectorAction::Duplicate);
        modes_per_action.push(common);
    }
    if !args.external_modes.is_empty() {
        actions.push(SelectorAction::ExternalOnly);
        modes_per_action.push(args.external_modes.clone());
        actions.push(SelectorAction::Extend);
        modes_per_action.push(args.external_modes.clone());
    }
    actions.push(SelectorAction::Cancel);
    modes_per_action.push(vec![]);
    SelectionContext {
        connector: args.connector.0.clone(),
        title,
        subtitle,
        actions,
        profile: args.profile.clone(),
        modes_per_action,
    }
}

/// Turn a [`Selection`] into a [`DisplayRequest`] (`None` = Cancel).
/// Manual mode must belong to the action's real options; otherwise `None`.
pub fn selection_to_request(
    ctx: &SelectionContext,
    action_idx: usize,
    sel: &Selection,
    position: Option<ExtendPosition>,
) -> Option<DisplayRequest> {
    let action = *ctx.actions.get(action_idx)?;
    if action == SelectorAction::Cancel || sel.action == SelectorAction::Cancel {
        return None;
    }
    let (kind, policy, requested) = match sel.action {
        SelectorAction::UseSavedProfile => {
            let p = ctx.profile.clone()?;
            (p.kind, p.policy, p.mode)
        }
        // Gaming migrates the whole desktop; the display part is external-only.
        SelectorAction::ExternalGaming => (PlanKind::ExternalOnly, sel.policy.clone(), sel.mode),
        SelectorAction::Duplicate => (PlanKind::Duplicate, sel.policy.clone(), sel.mode),
        SelectorAction::ExternalOnly => (PlanKind::ExternalOnly, sel.policy.clone(), sel.mode),
        SelectorAction::Extend => (PlanKind::Extend, sel.policy.clone(), sel.mode),
        SelectorAction::Cancel => return None,
    };
    // A manual mode is only valid if it is one of the action's real options.
    if let Some(m) = requested {
        let options = &ctx.modes_per_action[action_idx];
        if !options.iter().any(|o| o.matches_within(m, display_core::HZ_TOLERANCE)) {
            return None;
        }
    }
    Some(DisplayRequest {
        kind,
        target: Some(OutputId(ctx.connector.clone())),
        policy,
        requested_mode: requested,
        position,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use display_core::parse_mode;

    fn mode(s: &str) -> Mode {
        parse_mode(s).unwrap()
    }

    fn args() -> ContextArgs {
        ContextArgs {
            connector: OutputId("HDMI-A-1".into()),
            edid_name: Some("Samsung TV".into()),
            identity: None,
            current: None,
            internal_modes: vec![mode("1920x1080@60"), mode("1920x1080@120")],
            external_modes: vec![mode("3840x2160@120"), mode("1920x1080@120")],
            profile: Some(SavedProfile {
                alias: "samsung-tv".into(),
                kind: PlanKind::ExternalOnly,
                mode: Some(mode("1920x1080@120")),
                policy: ModePolicy::MaxRefresh,
            }),
        }
    }

    #[test]
    fn known_profile_offers_saved_option_first() {
        let ctx = build_context(&args());
        // Gaming leads (full session), saved profile right after.
        assert_eq!(ctx.actions[0], SelectorAction::ExternalGaming);
        assert_eq!(ctx.actions[1], SelectorAction::UseSavedProfile);
        assert_eq!(ctx.title, "Samsung TV");
        // Duplicate offered: 1080p common; modes are real intersect results.
        let dup_idx = ctx.actions.iter().position(|a| *a == SelectorAction::Duplicate).unwrap();
        assert_eq!(ctx.modes_per_action[dup_idx].len(), 1);
    }

    #[test]
    fn unknown_profile_has_no_saved_option() {
        let mut a = args();
        a.profile = None;
        let ctx = build_context(&a);
        assert!(!ctx.actions.contains(&SelectorAction::UseSavedProfile));
        assert!(ctx.actions.contains(&SelectorAction::Cancel));
    }

    #[test]
    fn no_common_mode_hides_duplicate() {
        let mut a = args();
        a.internal_modes = vec![mode("2560x1440@165")];
        let ctx = build_context(&a);
        assert!(!ctx.actions.contains(&SelectorAction::Duplicate));
        assert!(ctx.actions.contains(&SelectorAction::ExternalOnly));
    }

    #[test]
    fn no_external_modes_leaves_only_cancel() {
        let mut a = args();
        a.external_modes = vec![];
        a.profile = None;
        let ctx = build_context(&a);
        assert_eq!(ctx.actions, vec![SelectorAction::Cancel]);
    }

    #[test]
    fn selection_to_request_validates_manual_mode() {
        let ctx = build_context(&args());
        let ext_idx = ctx.actions.iter().position(|a| *a == SelectorAction::ExternalOnly).unwrap();
        let ok = Selection { action: SelectorAction::ExternalOnly, mode: Some(mode("3840x2160@120")), policy: ModePolicy::Manual(mode("3840x2160@120")) };
        let req = selection_to_request(&ctx, ext_idx, &ok, None).unwrap();
        assert_eq!(req.kind, PlanKind::ExternalOnly);
        let bad = Selection { action: SelectorAction::ExternalOnly, mode: Some(mode("2560x1440@165")), policy: ModePolicy::Manual(mode("2560x1440@165")) };
        assert!(selection_to_request(&ctx, ext_idx, &bad, None).is_none());
        let cancel = Selection { action: SelectorAction::Cancel, mode: None, policy: ModePolicy::MaxRefresh };
        assert!(selection_to_request(&ctx, ext_idx, &cancel, None).is_none());
    }

    #[test]
    fn saved_profile_becomes_request() {
        let ctx = build_context(&args());
        let sel = Selection { action: SelectorAction::UseSavedProfile, mode: None, policy: ModePolicy::MaxRefresh };
        let req = selection_to_request(&ctx, 0, &sel, None).unwrap();
        assert_eq!(req.kind, PlanKind::ExternalOnly);
        assert_eq!(req.requested_mode, Some(mode("1920x1080@120")));
    }
}
