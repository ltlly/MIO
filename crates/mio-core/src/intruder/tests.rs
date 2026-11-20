use super::*;

mod budgets;
mod generators;
mod grep;
mod payload_rules;
mod plan;

fn position(start: usize, end: usize) -> Position {
    Position { start, end }
}

fn sets(values: &[&[&[u8]]]) -> Vec<Vec<Vec<u8>>> {
    values
        .iter()
        .map(|set| set.iter().map(|value| value.to_vec()).collect())
        .collect()
}

fn requests(plan: &AttackPlan) -> Vec<Vec<u8>> {
    (0..plan.request_count())
        .map(|index| plan.request(index).unwrap())
        .collect()
}
