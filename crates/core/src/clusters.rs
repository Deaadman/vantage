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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deaths_join_the_first_spot_close_enough_on_the_same_floor() {
        let p = |x: f64, z: f64| Some([x, 0.0, z]);
        let points = [
            p(0.0, 0.0),
            p(1500.0, 0.0),  // within the radius: joins the first spot
            p(0.0, 400.0),   // right above it, another floor: a new spot
            None,            // no position: skipped
            p(2100.0, 0.0),  // past the radius from the seed, though near the second death
            p(-1999.0, 0.0), // just inside the radius
            p(0.0, 420.0),
            p(2000.0, 0.0),
            p(2200.0, 0.0),
        ];
        // The first spot has 4 (0, 1, 5, 7); the one at 2100 has 2; upstairs has 2.
        assert_eq!(leader_clusters(&points), vec![vec![0, 1, 5, 7]]);
        assert_eq!(centroid(&points, &[0, 1, 5, 7]), [375.0, 0.0, 0.0]);
        assert!(
            leader_clusters(&points[..3]).is_empty(),
            "fewer than MIN_DEATHS is no spot"
        );
    }

    #[test]
    fn bigger_spots_come_first_and_ties_keep_time_order() {
        let at = |x: f64| Some([x, 0.0, 0.0]);
        let points = [
            at(0.0),
            at(9000.0),
            at(0.0),
            at(9000.0),
            at(0.0),
            at(9000.0),
            at(9000.0),
        ];
        assert_eq!(leader_clusters(&points), vec![vec![1, 3, 5, 6], vec![0, 2, 4]]);
        let tie = [at(0.0), at(9000.0), at(0.0), at(9000.0), at(0.0), at(9000.0)];
        assert_eq!(leader_clusters(&tie), vec![vec![0, 2, 4], vec![1, 3, 5]]);
    }
}
