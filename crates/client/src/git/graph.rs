//! Lanes for the commit graph (`git/graph.ts`). Commits come newest first; each row says which
//! lane its commit sits in and which lines cross the row: lines passing straight through, lines
//! from above that end at the commit, and lines from the commit down to its parents' lanes.

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GraphRow {
    pub lane: usize,
    /// A line comes down into the commit in its own lane.
    pub top: bool,
    pub through: Vec<usize>,
    pub into: Vec<usize>,
    pub out: Vec<usize>,
    pub width: usize,
}

pub fn graph_rows<T>(
    commits: &[T],
    sha: impl Fn(&T) -> &str,
    parents: impl Fn(&T) -> &[String],
) -> Vec<GraphRow> {
    let mut lanes: Vec<Option<String>> = Vec::new();
    let mut rows = Vec::with_capacity(commits.len());
    let free = |lanes: &mut Vec<Option<String>>| -> usize {
        if let Some(index) = lanes.iter().position(Option::is_none) {
            index
        } else {
            lanes.push(None);
            lanes.len() - 1
        }
    };
    for commit in commits {
        let c_sha = sha(commit);
        let found = lanes.iter().position(|v| v.as_deref() == Some(c_sha));
        let top = found.is_some();
        let into_all: Vec<usize> = lanes
            .iter()
            .enumerate()
            .filter_map(|(i, v)| (v.as_deref() == Some(c_sha)).then_some(i))
            .collect();
        let lane = found.unwrap_or_else(|| free(&mut lanes));
        let through: Vec<usize> = lanes
            .iter()
            .enumerate()
            .filter_map(|(i, v)| v.as_deref().filter(|s| *s != c_sha).map(|_| i))
            .collect();
        for &index in &into_all {
            lanes[index] = None;
        }
        let mut out = Vec::new();
        for (position, parent) in parents(commit).iter().enumerate() {
            if let Some(existing) = lanes
                .iter()
                .position(|v| v.as_deref() == Some(parent.as_str()))
            {
                out.push(existing);
                continue;
            }
            let target = if position == 0 {
                lane
            } else {
                free(&mut lanes)
            };
            if target >= lanes.len() {
                lanes.resize(target + 1, None);
            }
            lanes[target] = Some(parent.clone());
            out.push(target);
        }
        while lanes.last().is_some_and(Option::is_none) {
            lanes.pop();
        }
        let width = (lane + 1)
            .max(lanes.len())
            .max(through.iter().map(|i| i + 1).max().unwrap_or(0))
            .max(out.iter().map(|i| i + 1).max().unwrap_or(0));
        let into = into_all
            .into_iter()
            .filter(|&index| index != lane)
            .collect();
        rows.push(GraphRow {
            lane,
            top,
            through,
            into,
            out,
            width,
        });
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_commit_opens_and_closes_a_lane() {
        let commits = vec![
            ("m", vec!["p1".to_owned(), "p2".to_owned()]),
            ("p2", vec!["base".to_owned()]),
            ("p1", vec!["base".to_owned()]),
            ("base", vec![]),
        ];
        let rows = graph_rows(&commits, |c| c.0, |c| &c.1);
        assert_eq!(
            rows[0],
            GraphRow {
                lane: 0,
                top: false,
                through: vec![],
                into: vec![],
                out: vec![0, 1],
                width: 2
            }
        );
        assert_eq!(
            rows[1],
            GraphRow {
                lane: 1,
                top: true,
                through: vec![0],
                into: vec![],
                out: vec![1],
                width: 2
            }
        );
        assert_eq!(
            rows[2],
            GraphRow {
                lane: 0,
                top: true,
                through: vec![1],
                into: vec![],
                out: vec![1],
                width: 2
            }
        );
        assert_eq!(
            rows[3],
            GraphRow {
                lane: 1,
                top: true,
                through: vec![],
                into: vec![],
                out: vec![],
                width: 2
            }
        );
    }
}
