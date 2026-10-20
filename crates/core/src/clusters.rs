//! Repeated death spots.
//!
//! Leader clustering in time order: each death joins the first earlier spot
//! whose seed (the first death there) is within `RADIUS_CM` on the plane and
//! `FLOOR_TOLERANCE_CM` vertically; otherwise it seeds a new spot. Spots with
//! fewer than `MIN_DEATHS` deaths are not reported. Simple and deterministic,
//! and it reproduces the prototype's oracle; single-linkage at any radius did not.

use serde::{Deserialize, Serialize};

use crate::catalog::MapInfo;
use crate::deaths::{DeathFact, place, round_to};

pub const RADIUS_CM: f64 = 2000.0;
pub const FLOOR_TOLERANCE_CM: f64 = 150.0;
pub const MIN_DEATHS: usize = 3;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeathCluster {
    pub id: usize,
    pub deaths: usize,
    pub centroid_cm: [f64; 3],
    pub minimap_uv: Option<[f64; 2]>,
    pub nearest_callout: Option<String>,
    pub rounds: Vec<u32>,
}

/// Leader clustering over positions given in time order (see the module
/// docs). Returns groups of indices into `points` with at least
/// `MIN_DEATHS` members, biggest first; ties keep time order. `None` points
/// are skipped.
pub fn leader_clusters(points: &[Option<[f64; 3]>]) -> Vec<Vec<usize>> {
    let mut seeds: Vec<([f64; 3], Vec<usize>)> = Vec::new();
    for (i, p) in points.iter().enumerate() {
        let Some(p) = *p else { continue };
        let near = seeds
            .iter_mut()
            .find(|(s, _)| (p[0] - s[0]).hypot(p[1] - s[1]) <= RADIUS_CM && (p[2] - s[2]).abs() <= FLOOR_TOLERANCE_CM);
        match near {
            Some((_, members)) => members.push(i),
            None => seeds.push((p, vec![i])),
        }
    }
    let mut groups: Vec<Vec<usize>> = seeds
        .into_iter()
        .map(|(_, m)| m)
        .filter(|m| m.len() >= MIN_DEATHS)
        .collect();
    groups.sort_by_key(|m| std::cmp::Reverse(m.len()));
    groups
}

/// Mean position of a group.
pub fn centroid(points: &[Option<[f64; 3]>], members: &[usize]) -> [f64; 3] {
    let n = members.len().max(1) as f64;
    let mut c = [0.0; 3];
    for p in members.iter().filter_map(|&i| points[i]) {
        for (acc, v) in c.iter_mut().zip(p) {
            *acc += v;
        }
    }
    c.map(|v| round_to(v / n, 0))
}

/// Finds repeated death spots and writes `cluster_id` into the facts.
pub fn cluster_deaths(facts: &mut [DeathFact], map: Option<&MapInfo>) -> Vec<DeathCluster> {
    let points: Vec<Option<[f64; 3]>> = facts.iter().map(|f| f.pos_cm).collect();
    leader_clusters(&points)
        .into_iter()
        .enumerate()
        .map(|(id, members)| {
            for &i in &members {
                facts[i].cluster_id = Some(id);
            }
            let centroid_cm = centroid(&points, &members);
            let (minimap_uv, nearest_callout) = place(map, centroid_cm);
            DeathCluster {
                id,
                deaths: members.len(),
                centroid_cm,
                minimap_uv,
                nearest_callout,
                rounds: members.iter().filter_map(|&i| facts[i].round).collect(),
            }
        })
        .collect()
}
