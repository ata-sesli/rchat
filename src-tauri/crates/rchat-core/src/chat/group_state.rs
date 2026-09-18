use std::collections::{BTreeMap, BTreeSet};

use crate::network::gossip::{GroupRecordBody, GroupSettings, SignedGroupRecord};

pub const MAX_PENDING_RECORDS_PER_GROUP: i64 = 1024;
pub const MAX_PENDING_RECORDS_PER_AUTHOR: i64 = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GroupRecordDecision {
    Effective,
    AcceptedNoOp,
    PendingDependencies(Vec<String>),
    Rejected(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupDerivedState {
    pub group_id: String,
    pub name: String,
    pub image_hash: Option<String>,
    pub admin_peer_id: String,
    pub settings: GroupSettings,
    pub active_members: BTreeSet<String>,
    pub invited_members: BTreeSet<String>,
    pub membership_order: BTreeMap<String, (u64, String)>,
    pub dissolved: bool,
}

impl GroupDerivedState {
    pub fn automatic_successor_peer_id(&self) -> Option<String> {
        self.active_members
            .iter()
            .filter(|peer_id| *peer_id != &self.admin_peer_id)
            .min_by_key(|peer_id| {
                self.membership_order
                    .get(*peer_id)
                    .cloned()
                    .unwrap_or((u64::MAX, (*peer_id).clone()))
            })
            .cloned()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupEvaluation {
    pub state: Option<GroupDerivedState>,
    pub decisions: BTreeMap<String, GroupRecordDecision>,
    pub accepted_ids_by_counter: BTreeMap<u64, BTreeSet<String>>,
    pub effective_policy_ids_by_counter: BTreeMap<u64, Vec<String>>,
    frontier_ids: Vec<String>,
}

impl GroupEvaluation {
    pub fn next_frontier(&self) -> Option<(u64, Vec<String>)> {
        let (&counter, _) = self.accepted_ids_by_counter.last_key_value()?;
        Some((counter + 1, self.frontier_ids.clone()))
    }

    pub fn missing_dependency_ids(&self) -> Vec<String> {
        self.decisions
            .values()
            .filter_map(|decision| match decision {
                GroupRecordDecision::PendingDependencies(missing) => Some(missing),
                _ => None,
            })
            .flatten()
            .filter(|id| id.as_str() != "founder record")
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .take(64)
            .collect()
    }
}

pub fn evaluate_group_records(records: &[SignedGroupRecord]) -> GroupEvaluation {
    let mut decisions = BTreeMap::new();
    let mut unique = BTreeMap::<String, SignedGroupRecord>::new();

    for record in records {
        if let Err(reason) = record.validate_shape() {
            decisions.insert(
                record.id().to_string(),
                GroupRecordDecision::Rejected(reason.into()),
            );
            continue;
        }
        if !record.verify() {
            decisions.insert(
                record.id().to_string(),
                GroupRecordDecision::Rejected("invalid signature or protocol version".into()),
            );
            continue;
        }
        unique
            .entry(record.id().to_string())
            .and_modify(|existing| {
                let existing_json = serde_json::to_vec(existing).unwrap_or_default();
                let candidate_json = serde_json::to_vec(record).unwrap_or_default();
                if candidate_json < existing_json {
                    *existing = record.clone();
                }
            })
            .or_insert_with(|| record.clone());
    }

    for record in unique.values() {
        if record.lamport_counter() == 1
            && matches!(record.body(), GroupRecordBody::GroupCreated { .. })
            && crate::chat_kind::derive_group_chat_id(record.author_peer_id(), record.id())
                != record.group_id()
        {
            decisions.insert(
                record.id().to_string(),
                GroupRecordDecision::Rejected(
                    "founder record does not match the self-certifying group id".into(),
                ),
            );
        }
    }

    let canonical_group_id = unique
        .values()
        .filter(|record| {
            record.lamport_counter() == 1
                && record.unsigned.parents.is_empty()
                && matches!(record.body(), GroupRecordBody::GroupCreated { .. })
                && crate::chat_kind::derive_group_chat_id(record.author_peer_id(), record.id())
                    == record.group_id()
        })
        .min_by(|left, right| canonical_cmp(left, right))
        .map(|record| record.group_id().to_string());

    let Some(group_id) = canonical_group_id else {
        for record in unique.values() {
            decisions.entry(record.id().to_string()).or_insert_with(|| {
                GroupRecordDecision::PendingDependencies(vec!["founder record".into()])
            });
        }
        return GroupEvaluation {
            state: None,
            decisions,
            accepted_ids_by_counter: BTreeMap::new(),
            effective_policy_ids_by_counter: BTreeMap::new(),
            frontier_ids: Vec::new(),
        };
    };

    for record in unique.values() {
        if record.group_id() != group_id {
            decisions.insert(
                record.id().to_string(),
                GroupRecordDecision::Rejected("record belongs to a different group".into()),
            );
        } else if record.lamport_counter() == 1
            && matches!(record.body(), GroupRecordBody::GroupCreated { .. })
            && crate::chat_kind::derive_group_chat_id(record.author_peer_id(), record.id())
                != record.group_id()
        {
            decisions.insert(
                record.id().to_string(),
                GroupRecordDecision::Rejected(
                    "founder record does not match the self-certifying group id".into(),
                ),
            );
        }
    }

    let mut by_counter = BTreeMap::<u64, Vec<SignedGroupRecord>>::new();
    for record in unique.values() {
        if record.group_id() == group_id {
            by_counter
                .entry(record.lamport_counter())
                .or_default()
                .push(record.clone());
        }
    }
    for records in by_counter.values_mut() {
        records.sort_by(canonical_cmp);
    }

    let mut causally_accepted = BTreeSet::new();
    let mut branch_states = BTreeMap::<String, GroupDerivedState>::new();
    let mut accepted_ids_by_counter = BTreeMap::<u64, BTreeSet<String>>::new();

    for (counter, counter_records) in by_counter {
        let mut seen_authors = BTreeSet::new();

        for record in counter_records {
            let id = record.id().to_string();
            if decisions.contains_key(&id) {
                continue;
            }
            if !seen_authors.insert(record.author_peer_id().to_string()) {
                decisions.insert(
                    id,
                    GroupRecordDecision::Rejected(
                        "author already has a canonical record at this counter".into(),
                    ),
                );
                continue;
            }

            if counter == 1 {
                if !causally_accepted.is_empty()
                    || !record.unsigned.parents.is_empty()
                    || !matches!(record.body(), GroupRecordBody::GroupCreated { .. })
                {
                    decisions.insert(
                        id,
                        GroupRecordDecision::Rejected("invalid founder record".into()),
                    );
                    continue;
                }
                let Some(founder_state) = create_state(&record) else {
                    decisions.insert(
                        id,
                        GroupRecordDecision::Rejected("invalid founder record".into()),
                    );
                    continue;
                };
                branch_states.insert(id.clone(), founder_state);
            } else {
                if let Some(decision) =
                    validate_frontier(&record, &unique, &decisions, &causally_accepted)
                {
                    decisions.insert(id, decision);
                    continue;
                }
                let Some(mut causal_state) =
                    derive_causal_state(&record, &unique, &causally_accepted, &branch_states)
                else {
                    decisions.insert(
                        id,
                        GroupRecordDecision::PendingDependencies(vec!["founder record".into()]),
                    );
                    continue;
                };
                if let Err(reason) = authorize_record(&causal_state, &record) {
                    decisions.insert(id, GroupRecordDecision::Rejected(reason));
                    continue;
                }
                apply_record(&mut causal_state, &record);
                branch_states.insert(id.clone(), causal_state);
            }

            causally_accepted.insert(id.clone());
            accepted_ids_by_counter
                .entry(counter)
                .or_default()
                .insert(id.clone());
        }
    }

    let mut state = None;
    let mut effective_policy_ids_by_counter = BTreeMap::<u64, Vec<String>>::new();
    let mut accepted_records = causally_accepted
        .iter()
        .filter_map(|id| unique.get(id))
        .collect::<Vec<_>>();
    accepted_records.sort_by(|left, right| canonical_cmp(left, right));
    for record in accepted_records {
        let changed = if record.lamport_counter() == 1 {
            state = create_state(record);
            state.is_some()
        } else if let Some(state) = state.as_mut() {
            apply_record(state, record)
        } else {
            false
        };
        decisions.insert(
            record.id().to_string(),
            if changed {
                GroupRecordDecision::Effective
            } else {
                GroupRecordDecision::AcceptedNoOp
            },
        );
        if changed && is_policy_record(record.body()) {
            effective_policy_ids_by_counter
                .entry(record.lamport_counter())
                .or_default()
                .push(record.id().to_string());
        }
    }

    let frontier_ids = build_next_frontier(
        &unique,
        &causally_accepted,
        &effective_policy_ids_by_counter,
    );

    GroupEvaluation {
        state,
        decisions,
        accepted_ids_by_counter,
        effective_policy_ids_by_counter,
        frontier_ids,
    }
}

fn canonical_cmp(left: &SignedGroupRecord, right: &SignedGroupRecord) -> std::cmp::Ordering {
    left.lamport_counter()
        .cmp(&right.lamport_counter())
        .then_with(|| left.author_peer_id().cmp(right.author_peer_id()))
        .then_with(|| left.id().cmp(right.id()))
}

fn validate_frontier(
    record: &SignedGroupRecord,
    all_records: &BTreeMap<String, SignedGroupRecord>,
    decisions: &BTreeMap<String, GroupRecordDecision>,
    causally_accepted: &BTreeSet<String>,
) -> Option<GroupRecordDecision> {
    if record.unsigned.parents.is_empty() {
        return Some(GroupRecordDecision::Rejected(
            "non-founder record requires parents".into(),
        ));
    }

    let mut missing = Vec::new();
    for parent_id in &record.unsigned.parents {
        let Some(parent) = all_records.get(parent_id) else {
            missing.push(parent_id.clone());
            continue;
        };
        if parent.group_id() != record.group_id() {
            return Some(GroupRecordDecision::Rejected(
                "parent belongs to a different group".into(),
            ));
        }
        if parent.lamport_counter() >= record.lamport_counter() {
            return Some(GroupRecordDecision::Rejected(
                "parent counter must precede child counter".into(),
            ));
        }
        match decisions.get(parent_id) {
            Some(GroupRecordDecision::PendingDependencies(parent_missing)) => {
                missing.extend(parent_missing.iter().cloned());
            }
            Some(GroupRecordDecision::Rejected(_)) => {
                return Some(GroupRecordDecision::Rejected(
                    "record references a rejected parent".into(),
                ));
            }
            _ if causally_accepted.contains(parent_id) => {}
            _ => missing.push(parent_id.clone()),
        }
    }
    if !missing.is_empty() {
        missing.sort();
        missing.dedup();
        return Some(GroupRecordDecision::PendingDependencies(missing));
    }

    let authorization_parent = all_records
        .get(&record.unsigned.parents[0])
        .expect("all parents were resolved above");
    if authorization_parent.lamport_counter() + 1 != record.lamport_counter() {
        return Some(GroupRecordDecision::Rejected(
            "authorization parent must be at the immediately preceding counter".into(),
        ));
    }
    None
}

fn derive_causal_state(
    record: &SignedGroupRecord,
    all_records: &BTreeMap<String, SignedGroupRecord>,
    causally_accepted: &BTreeSet<String>,
    branch_states: &BTreeMap<String, GroupDerivedState>,
) -> Option<GroupDerivedState> {
    if record.unsigned.parents.len() == 1 {
        return branch_states.get(&record.unsigned.parents[0]).cloned();
    }

    let ancestor_ids = collect_ancestor_ids(&record.unsigned.parents, all_records);
    let mut ancestors = ancestor_ids
        .iter()
        .filter(|id| causally_accepted.contains(*id))
        .filter_map(|id| all_records.get(id))
        .collect::<Vec<_>>();
    ancestors.sort_by(|left, right| canonical_cmp(left, right));
    project_records(ancestors)
}

fn collect_ancestor_ids(
    roots: &[String],
    all_records: &BTreeMap<String, SignedGroupRecord>,
) -> BTreeSet<String> {
    let mut ancestors = BTreeSet::new();
    let mut pending = roots.to_vec();
    while let Some(id) = pending.pop() {
        if !ancestors.insert(id.clone()) {
            continue;
        }
        if let Some(record) = all_records.get(&id) {
            pending.extend(record.unsigned.parents.iter().cloned());
        }
    }
    ancestors
}

fn project_records(records: Vec<&SignedGroupRecord>) -> Option<GroupDerivedState> {
    let mut state = None;
    for record in records {
        if record.lamport_counter() == 1 {
            if state.is_none() {
                state = create_state(record);
            }
        } else if let Some(state) = state.as_mut() {
            apply_record(state, record);
        }
    }
    state
}

fn build_next_frontier(
    all_records: &BTreeMap<String, SignedGroupRecord>,
    causally_accepted: &BTreeSet<String>,
    effective_policy_ids_by_counter: &BTreeMap<u64, Vec<String>>,
) -> Vec<String> {
    let Some(primary) = causally_accepted
        .iter()
        .filter_map(|id| all_records.get(id))
        .max_by(|left, right| canonical_cmp(left, right))
    else {
        return Vec::new();
    };
    let primary_ancestors = collect_ancestor_ids(&primary.unsigned.parents, all_records);
    let mut parents = vec![primary.id().to_string()];
    let mut outstanding_policy = effective_policy_ids_by_counter
        .values()
        .flatten()
        .filter(|id| id.as_str() != primary.id() && !primary_ancestors.contains(*id))
        .filter_map(|id| all_records.get(id))
        .collect::<Vec<_>>();
    outstanding_policy.sort_by(|left, right| canonical_cmp(left, right));
    parents.extend(
        outstanding_policy
            .into_iter()
            .take(crate::network::gossip::MAX_GROUP_RECORD_PARENTS.saturating_sub(1))
            .map(|record| record.id().to_string()),
    );
    parents
}

fn authorize_record(state: &GroupDerivedState, record: &SignedGroupRecord) -> Result<(), String> {
    if state.dissolved {
        return Err("group is dissolved".into());
    }
    let author = record.author_peer_id();
    match record.body() {
        GroupRecordBody::GroupCreated { .. } => Err("group already has a founder".into()),
        GroupRecordBody::MemberInvited { .. } => {
            if author == state.admin_peer_id
                || (state.settings.members_can_invite && state.active_members.contains(author))
            {
                Ok(())
            } else {
                Err("author cannot invite members".into())
            }
        }
        GroupRecordBody::MemberJoined { peer_id } => {
            if author != peer_id {
                Err("member join must be self-authored".into())
            } else if !state.invited_members.contains(peer_id) {
                Err("member does not have a causally prior invitation".into())
            } else {
                Ok(())
            }
        }
        GroupRecordBody::MemberLeft { peer_id } => {
            if author != peer_id {
                Err("member leave must be self-authored".into())
            } else if author == state.admin_peer_id {
                Err("administrator must transfer before leaving".into())
            } else if !state.active_members.contains(peer_id) {
                Err("member is not active".into())
            } else {
                Ok(())
            }
        }
        GroupRecordBody::GroupRenamed { .. }
        | GroupRecordBody::GroupSettingsUpdated { .. }
        | GroupRecordBody::MemberRemoved { .. }
        | GroupRecordBody::AdminTransferred { .. }
        | GroupRecordBody::GroupDissolved => {
            if author != state.admin_peer_id {
                return Err("only the causally prior administrator may apply this record".into());
            }
            match record.body() {
                GroupRecordBody::MemberRemoved { peer_id } if peer_id == &state.admin_peer_id => {
                    Err("current administrator cannot be removed".into())
                }
                GroupRecordBody::AdminTransferred { new_admin_peer_id }
                    if new_admin_peer_id == &state.admin_peer_id
                        || !state.active_members.contains(new_admin_peer_id) =>
                {
                    Err("new administrator must be a different active member".into())
                }
                GroupRecordBody::GroupDissolved if state.active_members.len() != 1 => {
                    Err("group can only be dissolved by its sole active member".into())
                }
                _ => Ok(()),
            }
        }
        GroupRecordBody::Message { .. }
        | GroupRecordBody::Receipt { .. }
        | GroupRecordBody::Head { .. }
        | GroupRecordBody::FileAvailability { .. } => {
            if state.active_members.contains(author) {
                Ok(())
            } else {
                Err("author is not an active member".into())
            }
        }
    }
}

fn create_state(record: &SignedGroupRecord) -> Option<GroupDerivedState> {
    let GroupRecordBody::GroupCreated {
        name,
        settings,
        image_hash,
    } = record.body()
    else {
        return None;
    };
    let founder = record.author_peer_id().to_string();
    Some(GroupDerivedState {
        group_id: record.group_id().to_string(),
        name: name.clone(),
        image_hash: image_hash.clone(),
        admin_peer_id: founder.clone(),
        settings: settings.clone().unwrap_or_default(),
        active_members: BTreeSet::from([founder.clone()]),
        invited_members: BTreeSet::new(),
        membership_order: BTreeMap::from([(
            founder,
            (record.lamport_counter(), record.id().to_string()),
        )]),
        dissolved: false,
    })
}

fn apply_record(state: &mut GroupDerivedState, record: &SignedGroupRecord) -> bool {
    if authorize_record(state, record).is_err() {
        return false;
    }
    match record.body() {
        GroupRecordBody::GroupCreated { .. } => false,
        GroupRecordBody::MemberInvited { peer_id, .. } => {
            if state.active_members.contains(peer_id) {
                false
            } else {
                state.invited_members.insert(peer_id.clone())
            }
        }
        GroupRecordBody::MemberJoined { peer_id } => {
            if !state.invited_members.remove(peer_id) {
                return false;
            }
            state.active_members.insert(peer_id.clone());
            state.membership_order.insert(
                peer_id.clone(),
                (record.lamport_counter(), record.id().to_string()),
            );
            true
        }
        GroupRecordBody::MemberLeft { peer_id } | GroupRecordBody::MemberRemoved { peer_id } => {
            if peer_id == &state.admin_peer_id {
                return false;
            }
            let changed =
                state.active_members.remove(peer_id) | state.invited_members.remove(peer_id);
            state.membership_order.remove(peer_id);
            changed
        }
        GroupRecordBody::GroupRenamed { name } => {
            if state.name == *name {
                false
            } else {
                state.name = name.clone();
                true
            }
        }
        GroupRecordBody::GroupSettingsUpdated { settings } => {
            if state.settings == *settings {
                false
            } else {
                state.settings = settings.clone();
                true
            }
        }
        GroupRecordBody::AdminTransferred { new_admin_peer_id } => {
            if state.admin_peer_id == *new_admin_peer_id
                || !state.active_members.contains(new_admin_peer_id)
            {
                false
            } else {
                state.admin_peer_id = new_admin_peer_id.clone();
                true
            }
        }
        GroupRecordBody::GroupDissolved => {
            if record.author_peer_id() != state.admin_peer_id || state.active_members.len() != 1 {
                return false;
            }
            state.dissolved = true;
            state.active_members.clear();
            state.invited_members.clear();
            state.membership_order.clear();
            true
        }
        GroupRecordBody::Message { .. }
        | GroupRecordBody::Receipt { .. }
        | GroupRecordBody::Head { .. }
        | GroupRecordBody::FileAvailability { .. } => true,
    }
}

fn is_policy_record(body: &GroupRecordBody) -> bool {
    !matches!(
        body,
        GroupRecordBody::Message { .. }
            | GroupRecordBody::Receipt { .. }
            | GroupRecordBody::Head { .. }
            | GroupRecordBody::FileAvailability { .. }
    )
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use libp2p::{identity, PeerId};

    use crate::network::gossip::{
        GroupRecordBody, GroupSettings, SignedGroupRecord, MAX_GROUP_RECORD_PARENTS,
    };

    use super::{evaluate_group_records, GroupRecordDecision};

    thread_local! {
        static TEST_GROUP_ID: RefCell<Option<String>> = const { RefCell::new(None) };
    }

    fn peer_id(keypair: &identity::Keypair) -> String {
        PeerId::from_public_key(&keypair.public()).to_string()
    }

    fn record(
        keypair: &identity::Keypair,
        id: &str,
        counter: u64,
        parents: &[&str],
        body: GroupRecordBody,
    ) -> SignedGroupRecord {
        record_owned(
            keypair,
            id,
            counter,
            parents.iter().map(|parent| (*parent).to_string()).collect(),
            body,
        )
    }

    fn record_owned(
        keypair: &identity::Keypair,
        id: &str,
        counter: u64,
        parents: Vec<String>,
        body: GroupRecordBody,
    ) -> SignedGroupRecord {
        let group_id = TEST_GROUP_ID.with(|group_id| {
            if counter == 1 && matches!(&body, GroupRecordBody::GroupCreated { .. }) {
                let derived = crate::chat_kind::derive_group_chat_id(&peer_id(keypair), id);
                *group_id.borrow_mut() = Some(derived.clone());
                derived
            } else {
                group_id.borrow().clone().unwrap_or_else(|| {
                    crate::chat_kind::derive_group_chat_id(&peer_id(keypair), "unanchored")
                })
            }
        });
        SignedGroupRecord::new(
            keypair,
            group_id,
            id.to_string(),
            counter as i64,
            parents,
            counter,
            body,
        )
        .unwrap()
    }

    fn fixture() -> (Vec<SignedGroupRecord>, String, String) {
        let founder = identity::Keypair::generate_ed25519();
        let member = identity::Keypair::generate_ed25519();
        let founder_id = peer_id(&founder);
        let member_id = peer_id(&member);
        let created = record(
            &founder,
            "created",
            1,
            &[],
            GroupRecordBody::GroupCreated {
                name: "Test".to_string(),
                settings: None,
                image_hash: None,
            },
        );
        let invited = record(
            &founder,
            "invited",
            2,
            &["created"],
            GroupRecordBody::MemberInvited {
                peer_id: member_id.clone(),
                role: "member".to_string(),
            },
        );
        let joined = record(
            &member,
            "joined",
            3,
            &["invited"],
            GroupRecordBody::MemberJoined {
                peer_id: member_id.clone(),
            },
        );
        (vec![created, invited, joined], founder_id, member_id)
    }

    fn permutations(records: &[SignedGroupRecord]) -> Vec<Vec<SignedGroupRecord>> {
        fn visit(
            remaining: &mut Vec<SignedGroupRecord>,
            current: &mut Vec<SignedGroupRecord>,
            output: &mut Vec<Vec<SignedGroupRecord>>,
        ) {
            if remaining.is_empty() {
                output.push(current.clone());
                return;
            }
            for index in 0..remaining.len() {
                let record = remaining.remove(index);
                current.push(record);
                visit(remaining, current, output);
                remaining.insert(index, current.pop().expect("current record"));
            }
        }

        let mut output = Vec::new();
        visit(&mut records.to_vec(), &mut Vec::new(), &mut output);
        output
    }

    #[test]
    fn record_arrival_order_does_not_change_state() {
        let (records, founder_id, member_id) = fixture();
        let permutations = [
            vec![0, 1, 2],
            vec![0, 2, 1],
            vec![1, 0, 2],
            vec![1, 2, 0],
            vec![2, 0, 1],
            vec![2, 1, 0],
        ];
        let expected = evaluate_group_records(&records);

        for permutation in permutations {
            let shuffled = permutation
                .into_iter()
                .map(|index| records[index].clone())
                .collect::<Vec<_>>();
            assert_eq!(evaluate_group_records(&shuffled), expected);
        }

        let state = expected.state.unwrap();
        assert_eq!(state.admin_peer_id, founder_id);
        assert!(state.active_members.contains(&member_id));
    }

    #[test]
    fn next_frontier_includes_every_effective_policy_record() {
        let (records, _, _) = fixture();
        let evaluation = evaluate_group_records(&records);

        assert_eq!(evaluation.next_frontier(), Some((4, vec!["joined".into()])));
    }

    #[test]
    fn malformed_record_shape_is_rejected_before_dependency_processing() {
        let founder = identity::Keypair::generate_ed25519();
        let oversized_frontier = record_owned(
            &founder,
            "too-many-parents",
            2,
            (0..=MAX_GROUP_RECORD_PARENTS)
                .map(|index| format!("parent-{index}"))
                .collect(),
            GroupRecordBody::Head { heads: Vec::new() },
        );

        let evaluation = evaluate_group_records(&[oversized_frontier]);
        assert!(matches!(
            evaluation.decisions.get("too-many-parents"),
            Some(GroupRecordDecision::Rejected(reason)) if reason.contains("too many parents")
        ));
    }

    #[test]
    fn competing_founder_cannot_take_over_a_self_certifying_group_id() {
        let founder = identity::Keypair::generate_ed25519();
        let attacker = identity::Keypair::generate_ed25519();
        let real_id = "real-genesis";
        let group_id = crate::chat_kind::derive_group_chat_id(&peer_id(&founder), real_id);
        let real = SignedGroupRecord::new(
            &founder,
            group_id.clone(),
            real_id.to_string(),
            1,
            Vec::new(),
            1,
            GroupRecordBody::GroupCreated {
                name: "Real".into(),
                settings: None,
                image_hash: None,
            },
        )
        .expect("real founder");
        let fake = SignedGroupRecord::new(
            &attacker,
            group_id,
            "fake-genesis".to_string(),
            1,
            Vec::new(),
            1,
            GroupRecordBody::GroupCreated {
                name: "Hijacked".into(),
                settings: None,
                image_hash: None,
            },
        )
        .expect("fake founder");

        for records in [vec![real.clone(), fake.clone()], vec![fake, real]] {
            let evaluation = evaluate_group_records(&records);
            let state = evaluation.state.expect("state");
            assert_eq!(state.admin_peer_id, peer_id(&founder));
            assert_eq!(state.name, "Real");
            assert!(matches!(
                evaluation.decisions.get("fake-genesis"),
                Some(GroupRecordDecision::Rejected(reason))
                    if reason.contains("self-certifying")
            ));
        }
    }

    #[test]
    fn same_counter_records_authorize_against_previous_counter_only() {
        let admin = identity::Keypair::generate_ed25519();
        let member = identity::Keypair::generate_ed25519();
        let member_id = peer_id(&member);
        let records = vec![
            record(
                &admin,
                "c",
                1,
                &[],
                GroupRecordBody::GroupCreated {
                    name: "Test".into(),
                    settings: None,
                    image_hash: None,
                },
            ),
            record(
                &admin,
                "i",
                2,
                &["c"],
                GroupRecordBody::MemberInvited {
                    peer_id: member_id.clone(),
                    role: "member".into(),
                },
            ),
            record(
                &member,
                "j",
                3,
                &["i"],
                GroupRecordBody::MemberJoined {
                    peer_id: member_id.clone(),
                },
            ),
            record(
                &admin,
                "transfer",
                4,
                &["j"],
                GroupRecordBody::AdminTransferred {
                    new_admin_peer_id: member_id.clone(),
                },
            ),
            record(
                &member,
                "rename-too-early",
                4,
                &["j"],
                GroupRecordBody::GroupRenamed {
                    name: "Too early".into(),
                },
            ),
        ];

        let evaluation = evaluate_group_records(&records);
        assert_eq!(evaluation.state.unwrap().admin_peer_id, member_id);
        assert!(matches!(
            evaluation.decisions.get("rename-too-early"),
            Some(GroupRecordDecision::Rejected(_))
        ));
        assert_eq!(
            evaluation.decisions.get("transfer"),
            Some(&GroupRecordDecision::Effective)
        );
    }

    #[test]
    fn concurrent_leave_cannot_make_same_counter_dissolution_valid() {
        let founder = identity::Keypair::generate_ed25519();
        let member = identity::Keypair::generate_ed25519();
        let admin_id = peer_id(&founder);
        let actual_member_id = peer_id(&member);
        let records = vec![
            record(
                &founder,
                "c",
                1,
                &[],
                GroupRecordBody::GroupCreated {
                    name: "Test".into(),
                    settings: None,
                    image_hash: None,
                },
            ),
            record(
                &founder,
                "i",
                2,
                &["c"],
                GroupRecordBody::MemberInvited {
                    peer_id: actual_member_id.clone(),
                    role: "member".into(),
                },
            ),
            record(
                &member,
                "j",
                3,
                &["i"],
                GroupRecordBody::MemberJoined {
                    peer_id: actual_member_id.clone(),
                },
            ),
            record(
                &member,
                "leave",
                4,
                &["j"],
                GroupRecordBody::MemberLeft {
                    peer_id: actual_member_id.clone(),
                },
            ),
            record(
                &founder,
                "dissolve",
                4,
                &["j"],
                GroupRecordBody::GroupDissolved,
            ),
        ];

        let evaluation = evaluate_group_records(&records);
        let state = evaluation.state.unwrap();
        assert_eq!(state.admin_peer_id, admin_id);
        assert!(!state.dissolved);
        assert!(!state.active_members.contains(&actual_member_id));
        assert!(matches!(
            evaluation.decisions.get("dissolve"),
            Some(GroupRecordDecision::Rejected(_))
        ));
    }

    #[test]
    fn concurrent_transfer_and_target_leave_preserve_an_active_administrator() {
        let founder = identity::Keypair::generate_ed25519();
        let member = identity::Keypair::generate_ed25519();
        let founder_id = peer_id(&founder);
        let member_id = peer_id(&member);
        let records = vec![
            record(
                &founder,
                "c",
                1,
                &[],
                GroupRecordBody::GroupCreated {
                    name: "Test".into(),
                    settings: None,
                    image_hash: None,
                },
            ),
            record(
                &founder,
                "i",
                2,
                &["c"],
                GroupRecordBody::MemberInvited {
                    peer_id: member_id.clone(),
                    role: "member".into(),
                },
            ),
            record(
                &member,
                "j",
                3,
                &["i"],
                GroupRecordBody::MemberJoined {
                    peer_id: member_id.clone(),
                },
            ),
            record(
                &founder,
                "transfer",
                4,
                &["j"],
                GroupRecordBody::AdminTransferred {
                    new_admin_peer_id: member_id.clone(),
                },
            ),
            record(
                &member,
                "leave",
                4,
                &["j"],
                GroupRecordBody::MemberLeft { peer_id: member_id },
            ),
        ];

        let expected = evaluate_group_records(&records);
        for permutation in permutations(&records) {
            assert_eq!(evaluate_group_records(&permutation), expected);
        }
        let state = expected.state.expect("state");
        assert!(state.active_members.contains(&state.admin_peer_id));
        assert!(state.admin_peer_id == founder_id || state.active_members.len() == 2);
    }

    #[test]
    fn late_concurrent_policy_record_does_not_invalidate_an_existing_descendant() {
        let founder = identity::Keypair::generate_ed25519();
        let successor = identity::Keypair::generate_ed25519();
        let inviter = identity::Keypair::generate_ed25519();
        let candidate = identity::Keypair::generate_ed25519();
        let successor_id = peer_id(&successor);
        let inviter_id = peer_id(&inviter);
        let candidate_id = peer_id(&candidate);
        let mut records = vec![
            record(
                &founder,
                "created",
                1,
                &[],
                GroupRecordBody::GroupCreated {
                    name: "Test".into(),
                    settings: Some(GroupSettings {
                        members_can_invite: true,
                    }),
                    image_hash: None,
                },
            ),
            record(
                &founder,
                "invite-successor",
                2,
                &["created"],
                GroupRecordBody::MemberInvited {
                    peer_id: successor_id.clone(),
                    role: "member".into(),
                },
            ),
            record(
                &successor,
                "join-successor",
                3,
                &["invite-successor"],
                GroupRecordBody::MemberJoined {
                    peer_id: successor_id.clone(),
                },
            ),
            record(
                &founder,
                "invite-inviter",
                4,
                &["join-successor"],
                GroupRecordBody::MemberInvited {
                    peer_id: inviter_id.clone(),
                    role: "member".into(),
                },
            ),
            record(
                &inviter,
                "join-inviter",
                5,
                &["invite-inviter"],
                GroupRecordBody::MemberJoined {
                    peer_id: inviter_id,
                },
            ),
            record(
                &founder,
                "transfer",
                6,
                &["join-inviter"],
                GroupRecordBody::AdminTransferred {
                    new_admin_peer_id: successor_id.clone(),
                },
            ),
            record(
                &successor,
                "rename",
                7,
                &["transfer"],
                GroupRecordBody::GroupRenamed {
                    name: "Renamed".into(),
                },
            ),
        ];

        let before = evaluate_group_records(&records);
        assert_eq!(
            before.decisions.get("rename"),
            Some(&GroupRecordDecision::Effective)
        );

        records.push(record(
            &inviter,
            "late-concurrent-invite",
            6,
            &["join-inviter"],
            GroupRecordBody::MemberInvited {
                peer_id: candidate_id,
                role: "member".into(),
            },
        ));

        let after = evaluate_group_records(&records);
        assert_eq!(
            after.decisions.get("late-concurrent-invite"),
            Some(&GroupRecordDecision::Effective)
        );
        assert_eq!(
            after.decisions.get("rename"),
            Some(&GroupRecordDecision::Effective),
            "a causally unrelated late record must not retroactively invalidate a descendant"
        );
        assert_eq!(after.state.expect("state").name, "Renamed");
    }

    #[test]
    fn bounded_policy_frontier_remains_advanceable() {
        let founder = identity::Keypair::generate_ed25519();
        let mut records = vec![record(
            &founder,
            "created",
            1,
            &[],
            GroupRecordBody::GroupCreated {
                name: "Test".into(),
                settings: None,
                image_hash: None,
            },
        )];
        let mut parent = "created".to_string();
        let mut counter = 2;
        let mut members = Vec::new();
        for index in 0..65 {
            let member = identity::Keypair::generate_ed25519();
            let member_id = peer_id(&member);
            let invite_id = format!("invite-{index}");
            records.push(record_owned(
                &founder,
                &invite_id,
                counter,
                vec![parent],
                GroupRecordBody::MemberInvited {
                    peer_id: member_id.clone(),
                    role: "member".into(),
                },
            ));
            counter += 1;
            let join_id = format!("join-{index}");
            records.push(record_owned(
                &member,
                &join_id,
                counter,
                vec![invite_id],
                GroupRecordBody::MemberJoined { peer_id: member_id },
            ));
            parent = join_id;
            counter += 1;
            members.push(member);
        }
        for (index, member) in members.iter().enumerate() {
            records.push(record_owned(
                member,
                &format!("leave-{index}"),
                counter,
                vec![parent.clone()],
                GroupRecordBody::MemberLeft {
                    peer_id: peer_id(member),
                },
            ));
        }

        let evaluation = evaluate_group_records(&records);
        let (next_counter, parents) = evaluation.next_frontier().expect("frontier");
        assert_eq!(next_counter, counter + 1);
        assert_eq!(parents.len(), MAX_GROUP_RECORD_PARENTS);
        assert_eq!(
            evaluation
                .decisions
                .values()
                .filter(|decision| matches!(decision, GroupRecordDecision::Rejected(_)))
                .count(),
            0
        );

        records.push(record_owned(
            &founder,
            "head-after-cap",
            next_counter,
            parents,
            GroupRecordBody::Head { heads: Vec::new() },
        ));
        let after_first_merge = evaluate_group_records(&records);
        assert_eq!(
            after_first_merge.decisions.get("head-after-cap"),
            Some(&GroupRecordDecision::Effective)
        );
        let (merge_counter, remaining_parents) = after_first_merge
            .next_frontier()
            .expect("remaining frontier");
        assert_eq!(remaining_parents.len(), 2);
        records.push(record_owned(
            &founder,
            "head-after-remainder",
            merge_counter,
            remaining_parents,
            GroupRecordBody::Head { heads: Vec::new() },
        ));
        assert_eq!(
            evaluate_group_records(&records).next_frontier(),
            Some((merge_counter + 1, vec!["head-after-remainder".into()]))
        );
    }
}
