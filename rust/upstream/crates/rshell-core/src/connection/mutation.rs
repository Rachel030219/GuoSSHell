use super::{
    CatalogMutation, CatalogOutcome, ConnectionCatalog, ConnectionGroup, ConnectionId,
    ConnectionProfile, DomainError, GroupId,
};

pub(crate) fn apply_mutation(
    catalog: &mut ConnectionCatalog,
    mutation: CatalogMutation,
) -> Result<CatalogOutcome, DomainError> {
    match mutation {
        CatalogMutation::Create(profile) => {
            if catalog.connections.contains_key(&profile.id) {
                return Err(DomainError::DuplicateConnection(profile.id));
            }
            let id = profile.id;
            insert_connection(catalog, profile);
            Ok(CatalogOutcome::Connection(id))
        }
        CatalogMutation::Update(profile) => {
            if catalog.connections.remove(&profile.id).is_none() {
                return Err(DomainError::ConnectionNotFound(profile.id));
            }
            insert_connection(catalog, profile);
            Ok(CatalogOutcome::Updated)
        }
        CatalogMutation::Duplicate {
            source,
            destination,
        } => {
            let source = catalog
                .connections
                .get(&source)
                .cloned()
                .ok_or(DomainError::ConnectionNotFound(source))?;
            let mut duplicate = source.clone();
            duplicate.id = ConnectionId::new();
            duplicate.group_id = destination;
            duplicate.position = i64::MAX;
            duplicate.name = copy_name(&source.name);
            let id = duplicate.id;
            insert_connection(catalog, duplicate);
            Ok(CatalogOutcome::Connection(id))
        }
        CatalogMutation::Move {
            connection,
            destination,
            position,
        } => {
            let mut profile = catalog
                .connections
                .remove(&connection)
                .ok_or(DomainError::ConnectionNotFound(connection))?;
            profile.group_id = destination;
            profile.position = usize_to_i64(position);
            insert_connection(catalog, profile);
            Ok(CatalogOutcome::Updated)
        }
        CatalogMutation::Delete(id) => {
            if catalog.connections.remove(&id).is_none() {
                return Err(DomainError::ConnectionNotFound(id));
            }
            Ok(CatalogOutcome::Deleted)
        }
        CatalogMutation::CreateGroup(group) => {
            if catalog.groups.contains_key(&group.id) {
                return Err(DomainError::DuplicateGroup(group.id));
            }
            let id = group.id;
            insert_group(catalog, group);
            Ok(CatalogOutcome::Group(id))
        }
        CatalogMutation::RenameGroup { group, name } => {
            let group = catalog
                .groups
                .get_mut(&group)
                .ok_or(DomainError::GroupNotFound(group))?;
            group.name = name;
            Ok(CatalogOutcome::Updated)
        }
        CatalogMutation::MoveGroup {
            group,
            parent,
            position,
        } => {
            let mut group = catalog
                .groups
                .remove(&group)
                .ok_or(DomainError::GroupNotFound(group))?;
            group.parent_id = parent;
            group.position = usize_to_i64(position);
            insert_group(catalog, group);
            Ok(CatalogOutcome::Updated)
        }
        CatalogMutation::DeleteGroup(id) => {
            delete_group(catalog, id)?;
            Ok(CatalogOutcome::Deleted)
        }
        CatalogMutation::SetTags { connection, tags } => {
            let profile = catalog
                .connections
                .get_mut(&connection)
                .ok_or(DomainError::ConnectionNotFound(connection))?;
            profile.tags = tags;
            Ok(CatalogOutcome::Updated)
        }
    }
}

fn delete_group(catalog: &mut ConnectionCatalog, id: GroupId) -> Result<(), DomainError> {
    if !catalog.groups.contains_key(&id) {
        return Err(DomainError::GroupNotFound(id));
    }
    if catalog
        .groups
        .values()
        .any(|group| group.parent_id == Some(id))
        || catalog
            .connections
            .values()
            .any(|profile| profile.group_id == Some(id))
    {
        return Err(DomainError::GroupNotEmpty { group_id: id });
    }
    catalog.groups.remove(&id);
    Ok(())
}

fn insert_group(catalog: &mut ConnectionCatalog, mut group: ConnectionGroup) {
    let position = bounded_position(
        group.position,
        catalog
            .groups
            .values()
            .filter(|item| item.parent_id == group.parent_id)
            .count(),
    );
    for existing in catalog
        .groups
        .values_mut()
        .filter(|item| item.parent_id == group.parent_id && item.position >= position)
    {
        existing.position = existing.position.saturating_add(1);
    }
    group.position = position;
    catalog.groups.insert(group.id, group);
}

fn insert_connection(catalog: &mut ConnectionCatalog, mut profile: ConnectionProfile) {
    let position = bounded_position(
        profile.position,
        catalog
            .connections
            .values()
            .filter(|item| item.group_id == profile.group_id)
            .count(),
    );
    for existing in catalog
        .connections
        .values_mut()
        .filter(|item| item.group_id == profile.group_id && item.position >= position)
    {
        existing.position = existing.position.saturating_add(1);
    }
    profile.position = position;
    catalog.connections.insert(profile.id, profile);
}

fn bounded_position(position: i64, count: usize) -> i64 {
    position.clamp(0, usize_to_i64(count))
}

fn usize_to_i64(value: usize) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

fn copy_name(name: &str) -> String {
    if name.trim().is_empty() {
        "copy".into()
    } else {
        format!("{} copy", name.trim())
    }
}
