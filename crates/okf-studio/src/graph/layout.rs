//! Hand-rolled Fruchterman–Reingold force-directed layout, plus a radial
//! by-directory-depth fallback.
//!
//! Layouts are deterministic: initial positions come from a hash of the node
//! key rather than an RNG, so the same bundle always settles into the same
//! shape. Simulation is incremental — the caller budgets iterations per
//! frame — and positions persist across snapshot reloads so the picture does
//! not jump when a file changes.

use super::model::{EdgeKind, GraphModel, NodeKind};
use std::collections::HashMap;

/// The available layout algorithms, cycled with `L`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LayoutMode {
    /// Force-directed (Fruchterman–Reingold).
    #[default]
    Force,
    /// Radial rings by concept-id directory depth.
    Radial,
}

impl LayoutMode {
    /// The next mode in the cycle.
    #[must_use]
    pub const fn next(self) -> Self {
        match self {
            Self::Force => Self::Radial,
            Self::Radial => Self::Force,
        }
    }

    /// Display name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Force => "force",
            Self::Radial => "radial",
        }
    }
}

/// FNV-1a, used for deterministic position seeding.
fn fnv1a(s: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.as_bytes() {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// The layout state: node positions keyed by stable node key, plus the
/// simulated-annealing temperature.
#[derive(Clone, Debug)]
pub struct LayoutEngine {
    /// Positions in abstract layout space, keyed by node key.
    pub positions: HashMap<String, (f64, f64)>,
    temperature: f64,
    /// `true` while the simulation still runs on ticks.
    pub running: bool,
}

impl Default for LayoutEngine {
    fn default() -> Self {
        Self {
            positions: HashMap::new(),
            temperature: 1.0,
            running: true,
        }
    }
}

impl LayoutEngine {
    /// Ensures every node in the model has a position, seeding new nodes
    /// deterministically near their neighbors' centroid (or on a hash-derived
    /// ring when they have none), and re-heats the simulation.
    pub fn seed(&mut self, model: &GraphModel) {
        let missing: Vec<usize> = (0..model.nodes.len())
            .filter(|&i| !self.positions.contains_key(&model.nodes[i].key))
            .collect();
        for &i in &missing {
            let node = &model.nodes[i];
            let hash = fnv1a(&node.key);
            #[allow(clippy::cast_precision_loss)]
            let angle = (hash % 6283) as f64 / 1000.0;
            #[allow(clippy::cast_precision_loss)]
            let radius = 0.35 + ((hash >> 16) % 1000) as f64 / 2000.0;
            // Phantom (broken) nodes are pinned toward the periphery.
            let radius = if node.kind == NodeKind::Phantom {
                radius + 0.6
            } else {
                radius
            };
            let mut pos = (radius * angle.cos(), radius * angle.sin());
            // New nodes enter near their neighbors' centroid when possible.
            let mut cx = 0.0;
            let mut cy = 0.0;
            let mut count = 0;
            for edge in &model.edges {
                let other = if edge.from == i {
                    edge.to
                } else if edge.to == i {
                    edge.from
                } else {
                    continue;
                };
                if let Some(&(x, y)) = self.positions.get(&model.nodes[other].key) {
                    cx += x;
                    cy += y;
                    count += 1;
                }
            }
            if count > 0 {
                let jitter = 0.05 + f64::from(u32::try_from(hash % 100).unwrap_or(0)) / 1000.0;
                pos = (
                    cx / f64::from(count) + jitter * angle.cos(),
                    cy / f64::from(count) + jitter * angle.sin(),
                );
            }
            self.positions.insert(node.key.clone(), pos);
        }
        if !missing.is_empty() {
            self.temperature = self.temperature.max(0.3);
            self.running = true;
        }
    }

    /// Runs `iterations` Fruchterman–Reingold steps over the nodes selected
    /// by `included` (pass all-true for the full graph). Returns `false`
    /// once the layout has cooled and no further stepping is useful.
    #[allow(clippy::cast_precision_loss)]
    pub fn step(&mut self, model: &GraphModel, included: &[bool], iterations: usize) -> bool {
        let indices: Vec<usize> = (0..model.nodes.len())
            .filter(|&i| included.get(i).copied().unwrap_or(false))
            .collect();
        let n = indices.len();
        if n < 2 || !self.running || self.temperature < 0.005 {
            self.running = false;
            return false;
        }
        let area = 4.0;
        let k = (area / n as f64).sqrt();

        for _ in 0..iterations {
            let mut displacements: HashMap<usize, (f64, f64)> = HashMap::new();
            // Repulsion between every included pair; hubs repel harder.
            for (a_pos, &a) in indices.iter().enumerate() {
                let pa = self.positions[&model.nodes[a].key];
                for &b in &indices[a_pos + 1..] {
                    let pb = self.positions[&model.nodes[b].key];
                    let (mut dx, mut dy) = (pa.0 - pb.0, pa.1 - pb.1);
                    let mut distance = dx.hypot(dy);
                    if distance < 1e-6 {
                        // Deterministic nudge for coincident nodes.
                        dx = 1e-3;
                        dy = 1e-3;
                        distance = 1.5e-3;
                    }
                    let hub = 1.0
                        + (model.nodes[a].degree.max(model.nodes[b].degree) as f64).sqrt() / 4.0;
                    let force = k * k / distance * hub;
                    let (ux, uy) = (dx / distance * force, dy / distance * force);
                    let da = displacements.entry(a).or_insert((0.0, 0.0));
                    da.0 += ux;
                    da.1 += uy;
                    let db = displacements.entry(b).or_insert((0.0, 0.0));
                    db.0 -= ux;
                    db.1 -= uy;
                }
            }
            // Attraction along edges; derivation edges prefer shorter length.
            for edge in &model.edges {
                if !included.get(edge.from).copied().unwrap_or(false)
                    || !included.get(edge.to).copied().unwrap_or(false)
                {
                    continue;
                }
                let pa = self.positions[&model.nodes[edge.from].key];
                let pb = self.positions[&model.nodes[edge.to].key];
                let (dx, dy) = (pa.0 - pb.0, pa.1 - pb.1);
                let distance = dx.hypot(dy).max(1e-6);
                let ideal = if edge.kind == EdgeKind::Derivation {
                    k * 0.6
                } else {
                    k
                };
                let force = distance * distance / ideal;
                let (ux, uy) = (dx / distance * force, dy / distance * force);
                let da = displacements.entry(edge.from).or_insert((0.0, 0.0));
                da.0 += ux;
                da.1 += uy;
                let db = displacements.entry(edge.to).or_insert((0.0, 0.0));
                db.0 += ux;
                db.1 += uy;
            }
            // Apply displacement, clamped by temperature.
            let limit = self.temperature * 0.1;
            for &i in &indices {
                let Some(&(dx, dy)) = displacements.get(&i) else {
                    continue;
                };
                let len = dx.hypot(dy);
                if len < 1e-9 {
                    continue;
                }
                let scale = (len.min(limit)) / len;
                let Some(pos) = self.positions.get_mut(&model.nodes[i].key) else {
                    continue;
                };
                pos.0 = dx.mul_add(scale, pos.0);
                pos.1 = dy.mul_add(scale, pos.1);
            }
            self.temperature *= 0.98;
        }
        true
    }

    /// Replaces positions with a radial layout: concepts ring by directory
    /// depth, spread deterministically by key hash within each ring.
    #[allow(clippy::cast_precision_loss)]
    pub fn radial(&mut self, model: &GraphModel) {
        let mut by_depth: HashMap<usize, Vec<usize>> = HashMap::new();
        for (i, node) in model.nodes.iter().enumerate() {
            let depth = match node.kind {
                NodeKind::Phantom | NodeKind::Source => 9,
                _ => node.id.as_ref().map_or(1, |id| id.segments().len()),
            };
            by_depth.entry(depth).or_default().push(i);
        }
        let mut depths: Vec<usize> = by_depth.keys().copied().collect();
        depths.sort_unstable();
        for (ring, depth) in depths.iter().enumerate() {
            let members = &by_depth[depth];
            let radius = (ring as f64).mul_add(0.35, 0.25);
            let count = members.len().max(1) as f64;
            let mut ordered = members.clone();
            ordered.sort_by_key(|&i| model.nodes[i].key.clone());
            for (slot, &i) in ordered.iter().enumerate() {
                let angle = std::f64::consts::TAU * slot as f64 / count;
                self.positions.insert(
                    model.nodes[i].key.clone(),
                    (radius * angle.cos(), radius * angle.sin()),
                );
            }
        }
        self.running = false;
    }

    /// Pauses or resumes the simulation. Resuming re-heats it slightly.
    pub const fn toggle_running(&mut self) {
        self.running = !self.running;
        if self.running && self.temperature < 0.05 {
            self.temperature = 0.2;
        }
    }
}
