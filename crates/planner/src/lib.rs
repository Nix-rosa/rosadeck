//! `rosadeck-planner`: pure `DisplayRequest -> DisplayPlan` construction.
//!
//! No IO, no sockets, no sysfs. The planner only reasons about
//! [`display_core`] types plus caller-supplied mode lists. Every plan is
//! validated before return, including the last-output rule:
//! a plan must never leave zero outputs enabled.

use display_core::{intersect_modes, parse_mode, select_mode, Mirror, Mode, ModePolicy, OutputId, PolicyContext, Position, HZ_TOLERANCE};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Requested disposition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PlanKind {
    /// Same mode on internal + external, external mirrors internal.
    Duplicate,
    /// External on, internal off (configure-then-disable order).
    ExternalOnly,
    /// Both on, side by side.
    Extend,
}

/// Relative position of the external output for [`PlanKind::Extend`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ExtendPosition {
    /// External to the right of internal.
    Right,
    /// External to the left of internal.
    Left,
    /// External above internal (negative y; Hyprland inverse-Y).
    Up,
    /// External below internal.
    Down,
}

/// High-level user intent (no Hyprland commands here).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DisplayRequest {
    /// What to build.
    pub kind: PlanKind,
    /// Target external output (`None` = auto-pick first connected external).
    pub target: Option<OutputId>,
    /// Selection policy for the shared/new mode.
    pub policy: ModePolicy,
    /// Explicit mode for `Manual` (must exist among candidates).
    pub requested_mode: Option<Mode>,
    /// Placement for extend (default `Right`).
    pub position: Option<ExtendPosition>,
}

/// One structured Hyprland mutation (serialized by the executor, not here).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MonitorCommand {
    /// Output to touch.
    pub output: OutputId,
    /// `WxH@Hz` (`None` = keep current mode).
    pub resolution: Option<String>,
    /// Layout position (`None` = keep).
    pub position: Option<Position>,
    /// Scale (`None` = keep; planner never invents scales).
    pub scale: Option<f64>,
    /// Mirror source. `None` means unmirrored: re-applying a rule without a
    /// mirror parameter clears mirroring (verified live against 0.56.2).
    /// Never serialize an empty mirror value: `,mirror,` poisons subsequent
    /// mirror rules in the same batch (observed: later mirrors silently lost).
    pub mirror: Option<OutputId>,
    /// Disable this output.
    pub disabled: bool,
    /// Rotation transform (`None` = keep; preserved from snapshot when set).
    pub transform: Option<i32>,
}

impl MonitorCommand {
    /// Controlled serialization to one `keyword monitor "<arg>"` argument.
    /// Format: `NAME,RES@RATE,XxY,SCALE[,mirror,OTHER][,transform,N]` or
    /// `NAME,disable`. Resolution Hz uses shortest roundtrip (`60`, `59.997`).
    pub fn to_keyword_arg(&self) -> String {
        if self.disabled {
            return format!("{},disable", self.output.0);
        }
        let res = self.resolution.clone().unwrap_or_else(|| "preferred".into());
        let pos = self.position.map(|p| format!("{}x{}", p.x, p.y)).unwrap_or_else(|| "auto".into());
        let scale = self.scale.map(|s| s.to_string()).unwrap_or_else(|| "auto".into());
        let mut arg = format!("{},{}", self.output.0, [res, pos, scale].join(","));
        if let Some(m) = &self.mirror {
            arg.push_str(&format!(",mirror,{}", m.0));
        }
        if let Some(t) = self.transform {
            arg.push_str(&format!(",transform,{t}"));
        }
        arg
    }
}

/// What `verify` must observe after apply.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExpectedOutput {
    /// Output name.
    pub id: OutputId,
    /// Expected active mode (`None` = must be disabled/absent-mode).
    pub mode: Option<Mode>,
    /// Expected disabled flag.
    pub disabled: bool,
    /// Expected mirror state.
    pub mirror: Mirror,
    /// Expected position (`None` = don't care).
    pub position: Option<Position>,
}

/// Validated, executable plan: ordered commands + expected end state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DisplayPlan {
    /// Requested disposition.
    pub kind: PlanKind,
    /// Commands in execution order (external configured BEFORE internal disabled).
    pub steps: Vec<MonitorCommand>,
    /// End state for `verify`.
    pub expect: Vec<ExpectedOutput>,
    /// Minimum enabled outputs afterwards (>= 1 always; == 1 for external-only).
    pub min_enabled: usize,
    /// Exact enabled count required (`Some(1)` for external-only).
    pub exact_enabled: Option<usize>,
}

/// Planner failure (all pre-apply).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanError {
    /// No connected external output found/selected.
    NoExternalOutput,
    /// No internal (laptop) output present.
    NoInternalOutput,
    /// Target output unknown or not connected.
    InvalidTarget(String),
    /// No usable modes for an involved output.
    NoModesAvailable(String),
    /// Empty mode intersection.
    NoCommonMode,
    /// Manual mode not among candidates.
    ManualModeUnavailable,
    /// Plan would leave zero outputs enabled (rejected pre-apply).
    WouldDisableLastOutput,
}

impl std::fmt::Display for PlanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoExternalOutput => write!(f, "NO_EXTERNAL_OUTPUT"),
            Self::NoInternalOutput => write!(f, "NO_INTERNAL_OUTPUT"),
            Self::InvalidTarget(t) => write!(f, "INVALID_TARGET: {t}"),
            Self::NoModesAvailable(o) => write!(f, "NO_MODES: {o}"),
            Self::NoCommonMode => write!(f, "NO_COMMON_MODE"),
            Self::ManualModeUnavailable => write!(f, "MANUAL_MODE_UNAVAILABLE"),
            Self::WouldDisableLastOutput => write!(f, "WOULD_DISABLE_LAST_OUTPUT"),
        }
    }
}

impl std::error::Error for PlanError {}

/// True for laptop panels (never a persistent identity, only role detection).
pub fn is_internal(name: &str) -> bool {
    name.starts_with("eDP-") || name.starts_with("LVDS")
}

/// Current per-output state the planner needs (from snapshot + live modes).
#[derive(Debug, Clone)]
pub struct PlanningOutput {
    /// Output name.
    pub id: OutputId,
    /// Connected right now.
    pub connected: bool,
    /// Currently enabled in the compositor.
    pub enabled: bool,
    /// Current mode, if any.
    pub current: Option<Mode>,
    /// Current position.
    pub position: Position,
    /// Current scale.
    pub scale: f64,
    /// Current transform (preserved by plans).
    pub transform: i32,
}

/// Everything `plan()` needs: states + applicable (Hyprland-side) modes.
#[derive(Debug, Clone, Default)]
pub struct PlanningInput {
    /// Known outputs.
    pub outputs: Vec<PlanningOutput>,
    /// Applicable modes per output name (compositor-accepted only).
    pub modes: HashMap<String, Vec<Mode>>,
}

fn modes_of(input: &PlanningInput, name: &str) -> Result<Vec<Mode>, PlanError> {
    let ms = input.modes.get(name).cloned().unwrap_or_default();
    if ms.is_empty() {
        return Err(PlanError::NoModesAvailable(name.into()));
    }
    Ok(ms)
}

fn pick_external(input: &PlanningInput, target: &Option<OutputId>) -> Result<PlanningOutput, PlanError> {
    let externals: Vec<&PlanningOutput> =
        input.outputs.iter().filter(|o| o.connected && !is_internal(&o.id.0)).collect();
    if let Some(t) = target {
        return externals
            .iter()
            .find(|o| o.id == *t)
            .cloned()
            .cloned()
            .ok_or_else(|| PlanError::InvalidTarget(t.0.clone()));
    }
    externals.into_iter().next().cloned().ok_or(PlanError::NoExternalOutput)
}

fn pick_internal(input: &PlanningInput) -> Option<PlanningOutput> {
    input.outputs.iter().find(|o| is_internal(&o.id.0)).cloned()
}

fn fmt_mode(m: &Mode) -> String {
    format!("{}x{}@{}", m.width, m.height, m.hz)
}

fn policy_with_manual(policy: &ModePolicy, requested: &Option<Mode>) -> ModePolicy {
    match (policy, requested) {
        (ModePolicy::Manual(_), Some(m)) => ModePolicy::Manual(*m),
        (ModePolicy::Manual(_), None) => ModePolicy::MaxRefresh,
        (p, _) => p.clone(),
    }
}

/// Build + validate a plan. Pure: no IO, deterministic.
pub fn plan(request: &DisplayRequest, input: &PlanningInput) -> Result<DisplayPlan, PlanError> {
    match request.kind {
        PlanKind::Duplicate => plan_duplicate(request, input),
        PlanKind::ExternalOnly => plan_external_only(request, input),
        PlanKind::Extend => plan_extend(request, input),
    }
}

fn plan_duplicate(request: &DisplayRequest, input: &PlanningInput) -> Result<DisplayPlan, PlanError> {
    let internal = pick_internal(input).ok_or(PlanError::NoInternalOutput)?;
    let external = pick_external(input, &request.target)?;
    let a = modes_of(input, &internal.id.0)?;
    let b = modes_of(input, &external.id.0)?;
    let common = intersect_modes(&a, &b);
    if common.is_empty() {
        return Err(PlanError::NoCommonMode);
    }
    let policy = policy_with_manual(&request.policy, &request.requested_mode);
    let ctx = PolicyContext { internal: &a, external: &b };
    let chosen =
        select_mode(&common, &policy, Some(&ctx)).ok_or(PlanError::ManualModeUnavailable)?;
    let res = fmt_mode(&chosen);
    let pos = internal.position;
    let plan = DisplayPlan {
        kind: PlanKind::Duplicate,
        steps: vec![
            MonitorCommand {
                output: internal.id.clone(),
                resolution: Some(res.clone()),
                position: Some(pos),
                scale: Some(internal.scale),
                mirror: None,
                disabled: false,
                transform: (internal.transform != 0).then_some(internal.transform),
            },
            MonitorCommand {
                output: external.id.clone(),
                resolution: Some(res.clone()),
                position: Some(pos),
                scale: None,
                mirror: Some(internal.id.clone()),
                disabled: false,
                transform: None,
            },
        ],
        expect: vec![
            ExpectedOutput { id: internal.id.clone(), mode: Some(chosen), disabled: false, mirror: Mirror::Disabled, position: Some(pos) },
            ExpectedOutput { id: external.id.clone(), mode: Some(chosen), disabled: false, mirror: Mirror::MirrorOf(internal.id.clone()), position: Some(pos) },
        ],
        min_enabled: 2,
        exact_enabled: None,
    };
    validate_last_output(&plan, input)?;
    Ok(plan)
}

fn plan_external_only(request: &DisplayRequest, input: &PlanningInput) -> Result<DisplayPlan, PlanError> {
    let internal = pick_internal(input).ok_or(PlanError::NoInternalOutput)?;
    let external = pick_external(input, &request.target)?;
    let b = modes_of(input, &external.id.0)?;
    let a = modes_of(input, &internal.id.0).unwrap_or_default();
    let policy = policy_with_manual(&request.policy, &request.requested_mode);
    let ctx = PolicyContext { internal: &a, external: &b };
    // Prefer modes the external side actually offers; match-policies may
    // legitimately resolve against the internal set, so fall back to it.
    let chosen = select_mode(&b, &policy, Some(&ctx))
        .or_else(|| select_mode(&a, &policy, Some(&ctx)))
        .ok_or(PlanError::ManualModeUnavailable)?;
    if policy == ModePolicy::Manual(request.requested_mode.unwrap_or(chosen)) && !b.iter().any(|m| m.matches_within(chosen, HZ_TOLERANCE)) {
        return Err(PlanError::ManualModeUnavailable);
    }
    let plan = DisplayPlan {
        kind: PlanKind::ExternalOnly,
        // Order is the safety core: external verified BEFORE internal disabled.
        steps: vec![
            MonitorCommand {
                output: external.id.clone(),
                resolution: Some(fmt_mode(&chosen)),
                position: Some(Position { x: 0, y: 0 }),
                scale: None,
                mirror: None,
                disabled: false,
                transform: None,
            },
            MonitorCommand { output: internal.id.clone(), resolution: None, position: None, scale: None, mirror: None, disabled: true, transform: None },
        ],
        expect: vec![
            ExpectedOutput { id: external.id.clone(), mode: Some(chosen), disabled: false, mirror: Mirror::Disabled, position: Some(Position { x: 0, y: 0 }) },
            ExpectedOutput { id: internal.id.clone(), mode: None, disabled: true, mirror: Mirror::Disabled, position: None },
        ],
        min_enabled: 1,
        exact_enabled: Some(1),
    };
    validate_last_output(&plan, input)?;
    Ok(plan)
}

fn plan_extend(request: &DisplayRequest, input: &PlanningInput) -> Result<DisplayPlan, PlanError> {
    let internal = pick_internal(input).ok_or(PlanError::NoInternalOutput)?;
    let external = pick_external(input, &request.target)?;
    let a = modes_of(input, &internal.id.0)?;
    let b = modes_of(input, &external.id.0)?;
    let policy = policy_with_manual(&request.policy, &request.requested_mode);
    let ctx = PolicyContext { internal: &a, external: &b };
    let mode_i = select_mode(&a, &policy, Some(&ctx)).ok_or(PlanError::ManualModeUnavailable)?;
    let mode_e = select_mode(&b, &policy, Some(&ctx)).ok_or(PlanError::ManualModeUnavailable)?;
    let place = request.position.unwrap_or(ExtendPosition::Right);
    let (pi, pe) = match place {
        ExtendPosition::Right => (Position { x: 0, y: 0 }, Position { x: mode_i.width as i32, y: 0 }),
        ExtendPosition::Left => (Position { x: mode_e.width as i32, y: 0 }, Position { x: 0, y: 0 }),
        ExtendPosition::Down => (Position { x: 0, y: 0 }, Position { x: 0, y: mode_i.height as i32 }),
        ExtendPosition::Up => (Position { x: 0, y: mode_e.height as i32 }, Position { x: 0, y: 0 }),
    };
    let plan = DisplayPlan {
        kind: PlanKind::Extend,
        steps: vec![
            MonitorCommand { output: internal.id.clone(), resolution: Some(fmt_mode(&mode_i)), position: Some(pi), scale: Some(internal.scale), mirror: None, disabled: false, transform: (internal.transform != 0).then_some(internal.transform) },
            MonitorCommand { output: external.id.clone(), resolution: Some(fmt_mode(&mode_e)), position: Some(pe), scale: None, mirror: None, disabled: false, transform: None },
        ],
        expect: vec![
            ExpectedOutput { id: internal.id.clone(), mode: Some(mode_i), disabled: false, mirror: Mirror::Disabled, position: Some(pi) },
            ExpectedOutput { id: external.id.clone(), mode: Some(mode_e), disabled: false, mirror: Mirror::Disabled, position: Some(pe) },
        ],
        min_enabled: 2,
        exact_enabled: None,
    };
    validate_last_output(&plan, input)?;
    Ok(plan)
}

/// Reject any plan that would leave zero outputs enabled, counting outputs
/// the plan does not touch as remaining in their current state.
fn validate_last_output(plan: &DisplayPlan, input: &PlanningInput) -> Result<(), PlanError> {
    let mut enabled: HashMap<&str, bool> =
        input.outputs.iter().map(|o| (o.id.0.as_str(), o.enabled)).collect();
    for s in &plan.steps {
        enabled.insert(s.output.0.as_str(), !s.disabled);
    }
    if !enabled.values().any(|e| *e) {
        return Err(PlanError::WouldDisableLastOutput);
    }
    let after = enabled.values().filter(|e| **e).count();
    if after < plan.min_enabled {
        return Err(PlanError::WouldDisableLastOutput);
    }
    Ok(())
}

/// Parse `WxH@Hz` for `--mode` (exact Hz preserved; `Hz` suffix optional).
pub fn parse_requested_mode(s: &str) -> Option<Mode> {
    parse_mode(s).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mode(s: &str) -> Mode {
        parse_mode(s).unwrap()
    }

    fn lab_input() -> PlanningInput {
        // Laptop 2560x1440@165/144 + TV 4K120/1080p120/720p60 (all connected).
        let outputs = vec![
            PlanningOutput { id: OutputId("eDP-1".into()), connected: true, enabled: true, current: Some(mode("2560x1440@165")), position: Position { x: 0, y: 0 }, scale: 1.0, transform: 0 },
            PlanningOutput { id: OutputId("HDMI-A-1".into()), connected: true, enabled: false, current: None, position: Position { x: 0, y: 0 }, scale: 1.0, transform: 0 },
        ];
        let modes = HashMap::from([
            ("eDP-1".into(), vec![mode("2560x1440@165"), mode("2560x1440@144"), mode("1920x1080@165"), mode("1920x1080@120")]),
            ("HDMI-A-1".into(), vec![mode("3840x2160@120"), mode("3840x2160@60"), mode("1920x1080@120"), mode("1920x1080@60"), mode("1280x720@60")]),
        ]);
        PlanningInput { outputs, modes }
    }

    fn req(kind: PlanKind) -> DisplayRequest {
        DisplayRequest { kind, target: None, policy: ModePolicy::MaxRefresh, requested_mode: None, position: None }
    }

    #[test]
    fn duplicate_uses_common_modes_and_mirrors() {
        let p = plan(&req(PlanKind::Duplicate), &lab_input()).unwrap();
        assert_eq!(p.steps.len(), 2);
        let ext = &p.steps[1];
        assert_eq!(ext.output.0, "HDMI-A-1");
        assert_eq!(ext.mirror.as_ref().unwrap().0, "eDP-1");
        assert!(!ext.disabled);
        // MaxRefresh over {1080p120, 1080p60} common -> 1080p120.
        assert_eq!(ext.resolution.as_deref(), Some("1920x1080@120"));
        assert_eq!(p.expect[1].mirror, Mirror::MirrorOf(OutputId("eDP-1".into())));
    }

    #[test]
    fn duplicate_no_common_mode() {
        let mut input = lab_input();
        input.modes.insert("HDMI-A-1".into(), vec![mode("1280x720@60")]);
        input.modes.insert("eDP-1".into(), vec![mode("2560x1440@165")]);
        assert_eq!(plan(&req(PlanKind::Duplicate), &input), Err(PlanError::NoCommonMode));
    }

    #[test]
    fn external_only_orders_configure_before_disable() {
        let p = plan(&req(PlanKind::ExternalOnly), &lab_input()).unwrap();
        assert_eq!(p.steps.len(), 2);
        assert!(!p.steps[0].disabled && p.steps[0].output.0 == "HDMI-A-1");
        assert!(p.steps[1].disabled && p.steps[1].output.0 == "eDP-1");
        assert_eq!(p.exact_enabled, Some(1));
        assert_eq!(p.steps[0].resolution.as_deref(), Some("3840x2160@120")); // MaxRefresh
    }

    #[test]
    fn external_only_rejects_disconnected_target() {
        let mut input = lab_input();
        input.outputs[1].connected = false;
        let r = DisplayRequest { target: Some(OutputId("HDMI-A-1".into())), ..req(PlanKind::ExternalOnly) };
        assert_eq!(plan(&r, &input), Err(PlanError::InvalidTarget("HDMI-A-1".into())));
    }

    #[test]
    fn external_only_no_external_at_all() {
        let mut input = lab_input();
        input.outputs.truncate(1);
        assert_eq!(plan(&req(PlanKind::ExternalOnly), &input), Err(PlanError::NoExternalOutput));
    }

    #[test]
    fn external_only_manual_must_exist_on_external() {
        let r = DisplayRequest { requested_mode: Some(mode("2560x1440@165")), policy: ModePolicy::Manual(mode("2560x1440@165")), ..req(PlanKind::ExternalOnly) };
        assert_eq!(plan(&r, &lab_input()), Err(PlanError::ManualModeUnavailable));
    }

    #[test]
    fn extend_positions_all_directions() {
        let mut input = lab_input();
        input.modes.insert("eDP-1".into(), vec![mode("1920x1080@60")]);
        input.modes.insert("HDMI-A-1".into(), vec![mode("1920x1080@60")]);
        for (place, expect_pe) in [
            (ExtendPosition::Right, Position { x: 1920, y: 0 }),
            (ExtendPosition::Left, Position { x: 0, y: 0 }),
            (ExtendPosition::Down, Position { x: 0, y: 1080 }),
            (ExtendPosition::Up, Position { x: 0, y: 0 }),
        ] {
            let r = DisplayRequest { position: Some(place), ..req(PlanKind::Extend) };
            let p = plan(&r, &input).unwrap();
            assert_eq!(p.steps[1].position, Some(expect_pe), "{place:?}");
            assert!(!p.steps[0].disabled && !p.steps[1].disabled);
        }
        // Left: internal shifted right by external width.
        let r = DisplayRequest { position: Some(ExtendPosition::Left), ..req(PlanKind::Extend) };
        assert_eq!(plan(&r, &input).unwrap().steps[0].position, Some(Position { x: 1920, y: 0 }));
    }

    #[test]
    fn extend_keeps_user_scales() {
        let p = plan(&req(PlanKind::Extend), &lab_input()).unwrap();
        assert_eq!(p.steps[0].scale, Some(1.0)); // internal kept
        assert_eq!(p.steps[1].scale, None); // external untouched
    }

    #[test]
    fn rejects_plan_leaving_zero_outputs() {
        // Both disabled afterwards: manual steps simulation via request that
        // disables the only outputs (external-only with internal already off
        // and external missing is NoExternalOutput; so craft direct check).
        let plan = DisplayPlan {
            kind: PlanKind::ExternalOnly,
            steps: vec![
                MonitorCommand { output: OutputId("eDP-1".into()), resolution: None, position: None, scale: None, mirror: None, disabled: true, transform: None },
                MonitorCommand { output: OutputId("HDMI-A-1".into()), resolution: None, position: None, scale: None, mirror: None, disabled: true, transform: None },
            ],
            expect: vec![],
            min_enabled: 1,
            exact_enabled: None,
        };
        let input = lab_input();
        assert_eq!(validate_last_output(&plan, &input), Err(PlanError::WouldDisableLastOutput));
    }

    #[test]
    fn multiple_externals_target_selection() {
        let mut input = lab_input();
        input.outputs.push(PlanningOutput { id: OutputId("DP-1".into()), connected: true, enabled: false, current: None, position: Position { x: 0, y: 0 }, scale: 1.0, transform: 0 });
        input.modes.insert("DP-1".into(), vec![mode("1920x1080@60")]);
        let auto = plan(&req(PlanKind::ExternalOnly), &input).unwrap();
        assert_eq!(auto.steps[0].output.0, "HDMI-A-1"); // first connected external
        let r = DisplayRequest { target: Some(OutputId("DP-1".into())), ..req(PlanKind::ExternalOnly) };
        assert_eq!(plan(&r, &input).unwrap().steps[0].output.0, "DP-1");
    }

    #[test]
    fn keyword_arg_serialization() {
        let c = MonitorCommand { output: OutputId("HDMI-A-1".into()), resolution: Some("1920x1080@120".into()), position: Some(Position { x: 0, y: 0 }), scale: Some(1.0), mirror: Some(OutputId("eDP-1".into())), disabled: false, transform: None };
        assert_eq!(c.to_keyword_arg(), "HDMI-A-1,1920x1080@120,0x0,1,mirror,eDP-1");
        let d = MonitorCommand { output: OutputId("eDP-1".into()), resolution: None, position: None, scale: None, mirror: None, disabled: true, transform: None };
        assert_eq!(d.to_keyword_arg(), "eDP-1,disable");
        let t = MonitorCommand { output: OutputId("DP-1".into()), resolution: Some("1920x1080@60".into()), position: None, scale: None, mirror: None, disabled: false, transform: Some(1) };
        assert_eq!(t.to_keyword_arg(), "DP-1,1920x1080@60,auto,auto,transform,1");
    }

    #[test]
    fn invalid_target_name() {
        let r = DisplayRequest { target: Some(OutputId("NOPE".into())), ..req(PlanKind::Duplicate) };
        assert_eq!(plan(&r, &lab_input()), Err(PlanError::InvalidTarget("NOPE".into())));
    }
}
