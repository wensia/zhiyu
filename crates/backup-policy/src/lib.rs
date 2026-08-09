use chrono::{DateTime, Duration, Utc};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub id: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetentionPlan {
    pub keep: Vec<Snapshot>,
    pub delete: Vec<Snapshot>,
}

/// 规划成功快照的保留与删除集合，不执行任何文件系统操作。
///
/// 保留窗口内的全部快照，并且无论多旧都额外保留最新一份。
pub fn plan_retention(
    snapshots: &[Snapshot],
    now: DateTime<Utc>,
    retention_days: i64,
) -> RetentionPlan {
    let cutoff = now - Duration::days(retention_days.max(0));
    let newest_id = snapshots
        .iter()
        .max_by(|left, right| {
            left.created_at
                .cmp(&right.created_at)
                .then_with(|| left.id.cmp(&right.id))
        })
        .map(|snapshot| snapshot.id.as_str());
    let (keep, delete) = snapshots.iter().cloned().partition(|snapshot| {
        snapshot.created_at >= cutoff || Some(snapshot.id.as_str()) == newest_id
    });
    RetentionPlan { keep, delete }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(id: &str, created_at: &str) -> Snapshot {
        Snapshot {
            id: id.into(),
            created_at: created_at.parse().unwrap(),
        }
    }

    #[test]
    fn keeps_every_snapshot_inside_the_window_including_boundary() {
        let now = "2026-08-10T12:00:00Z".parse().unwrap();
        let snapshots = vec![
            snapshot("old", "2026-07-11T11:59:59Z"),
            snapshot("boundary", "2026-07-11T12:00:00Z"),
            snapshot("recent", "2026-08-10T00:00:00Z"),
        ];

        let plan = plan_retention(&snapshots, now, 30);

        assert_eq!(
            plan.keep
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>(),
            vec!["boundary", "recent"]
        );
        assert_eq!(plan.delete, vec![snapshots[0].clone()]);
    }

    #[test]
    fn always_keeps_the_newest_successful_snapshot() {
        let now = "2026-08-10T12:00:00Z".parse().unwrap();
        let snapshots = vec![
            snapshot("older", "2026-01-01T00:00:00Z"),
            snapshot("newest", "2026-02-01T00:00:00Z"),
        ];

        let plan = plan_retention(&snapshots, now, 30);

        assert_eq!(plan.keep, vec![snapshots[1].clone()]);
        assert_eq!(plan.delete, vec![snapshots[0].clone()]);
    }
}
