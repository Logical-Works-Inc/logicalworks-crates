//! The model's side of a step: which grant a host draws, and what the modeled
//! lane it names must do next.

use lgwks_std::seeded::Seeded;

use super::{Model, ModelLane, Target, TestResult};

/// One grant's target drawn from the host's open modeled runs: a run and a
/// lane, ending or dispatching by draw.
pub(super) fn pick_grant(
    rng: &mut Seeded,
    model: &Model,
    hid: usize,
) -> TestResult<Option<Target>> {
    let ids: Vec<&String> = model
        .runs
        .iter()
        .filter(|&(_, run)| run.host == hid && run.close.is_none())
        .map(|(id, _)| id)
        .collect();
    if ids.is_empty() {
        return Ok(None);
    }
    let picked = rng.index(ids.len())?;
    let drawn = rng.below(3)?;
    let lane =
        usize::try_from(drawn).map_err(|error| format!("the lane draw does not fit: {error}"))?;
    let end = rng.below(2)? == 0;
    let run = ids
        .get(picked)
        .ok_or_else(|| format!("the draw {picked} names no open run of {}", ids.len()))?;
    Ok(Some(((*run).clone(), lane, end)))
}

/// Whether the modeled lane must dispatch: present and pending.
pub(super) fn lane_must_dispatch(model: &Model, run: &str, lane: usize) -> TestResult<bool> {
    let held = model
        .runs
        .get(run)
        .ok_or_else(|| format!("the model lost {run}"))?;
    Ok(held.lanes.get(lane) == Some(&ModelLane::Pending))
}

/// Whether the modeled lane must end: present and unended.
pub(super) fn lane_must_end(model: &Model, run: &str, lane: usize) -> TestResult<bool> {
    let held = model
        .runs
        .get(run)
        .ok_or_else(|| format!("the model lost {run}"))?;
    Ok(match held.lanes.get(lane) {
        None => false,
        Some(&ModelLane::Ended) => false,
        Some(_) => true,
    })
}

/// The modeled lane a step targets: the run and the lane row it names.
pub(super) fn model_lane_slot<'a>(
    model: &'a mut Model,
    run: &str,
    lane: usize,
) -> TestResult<&'a mut ModelLane> {
    let held = model
        .runs
        .get_mut(run)
        .ok_or_else(|| format!("the model lost {run}"))?;
    let Some(slot) = held.lanes.get_mut(lane) else {
        lgwks_std::trace::warn!(run = %run, lane, "sim_dist_lease: the model lost the lane a step targets");
        return Err(format!("the model lost lane {lane} of {run}").into());
    };
    Ok(slot)
}
