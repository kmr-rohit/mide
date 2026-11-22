//! Commit-graph lane assignment (the colored rail in the commit list).
//!
//! Feed commits in topological order (children before parents) to
//! [`GraphBuilder::push`]; each call returns the drawing instructions for that
//! row. A renderer draws, per row:
//! * for every lane in `incoming` that is *not* in `converging`: a straight
//!   line from the top edge to the middle of the row (pass-through),
//! * for every lane in `converging`: a line from the top edge at that lane to
//!   the node at `column`,
//! * the node at `column`,
//! * for every [`Edge`] in `outgoing`: a line from the middle of the row at
//!   `from` to the bottom edge at `to`.

use git2::Oid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Edge {
    pub from: usize,
    pub to: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GraphRow {
    /// Lane the commit node sits in.
    pub column: usize,
    /// Lanes that are active above this row.
    pub incoming: Vec<usize>,
    /// Subset of `incoming` that ends at this commit.
    pub converging: Vec<usize>,
    /// Lines leaving this row downward (pass-throughs and parent links).
    pub outgoing: Vec<Edge>,
    /// Number of lanes this row needs horizontally.
    pub width: usize,
}

#[derive(Default)]
pub struct GraphBuilder {
    lanes: Vec<Option<Oid>>,
}

impl GraphBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    fn free_slot(&mut self) -> usize {
        if let Some(i) = self.lanes.iter().position(Option::is_none) {
            i
        } else {
            self.lanes.push(None);
            self.lanes.len() - 1
        }
    }

    pub fn push(&mut self, id: Oid, parents: &[Oid]) -> GraphRow {
        let incoming: Vec<usize> = self
            .lanes
            .iter()
            .enumerate()
            .filter_map(|(i, l)| l.map(|_| i))
            .collect();
        let converging: Vec<usize> = self
            .lanes
            .iter()
            .enumerate()
            .filter_map(|(i, l)| (*l == Some(id)).then_some(i))
            .collect();

        let column = match converging.first() {
            Some(&c) => c,
            None => self.free_slot(),
        };
        for &i in &converging {
            self.lanes[i] = None;
        }

        // Lanes that simply continue past this commit.
        let mut outgoing: Vec<Edge> = self
            .lanes
            .iter()
            .enumerate()
            .filter_map(|(i, l)| l.map(|_| Edge { from: i, to: i }))
            .collect();

        for &p in parents {
            let to = match self.lanes.iter().position(|l| *l == Some(p)) {
                Some(j) => j, // parent already has a lane: merge into it
                None => {
                    let slot = if self.lanes[column].is_none() { column } else { self.free_slot() };
                    self.lanes[slot] = Some(p);
                    slot
                }
            };
            let edge = Edge { from: column, to };
            if !outgoing.contains(&edge) {
                outgoing.push(edge);
            }
        }

        while matches!(self.lanes.last(), Some(None)) {
            self.lanes.pop();
        }

        let width = incoming
            .iter()
            .copied()
            .chain(outgoing.iter().flat_map(|e| [e.from, e.to]))
            .chain([column])
            .max()
            .map_or(1, |m| m + 1);

        GraphRow { column, incoming, converging, outgoing, width }
    }
}
