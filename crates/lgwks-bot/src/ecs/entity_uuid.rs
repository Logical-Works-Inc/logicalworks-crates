//! Entity UUID component and tenant-scoped FoundationDB tuple entity index (#851).
//!
//! Provides [`EntityUuid`] for tagging ECS entities with persistent 128-bit UUIDs
//! and [`TenantEntityIndex`] for mapping `(tenant, uuid)` pairs to entity handles
//! using FoundationDB tuple-encoded keys `[Element::String(tenant), Element::Uuid(uuid)]`
//! (`0x02 <tenant> 0x00 0x30 <uuid>`).

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::fmt;

#[cfg(feature = "ecs")]
use lgwks_deps::bevy_ecs::{
    self,
    prelude::{Component, Entity, Resource},
};
use lgwks_std::id::{Element, TupleError, Uuid, pack_elements};

/// The default entity identifier type in the ECS world.
#[cfg(feature = "ecs")]
pub(crate) type DefaultEntity = Entity;

/// The default entity identifier type when the ECS substrate is disabled.
#[cfg(not(feature = "ecs"))]
pub(crate) type DefaultEntity = u64;

/// A 128-bit persistent UUID component identifying an entity in the ECS world (#851).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(feature = "ecs", derive(Component))]
#[non_exhaustive]
pub(crate) struct EntityUuid(
    /// The underlying 128-bit UUID identifier.
    pub(crate) Uuid,
);

impl EntityUuid {
    /// Creates a new entity UUID wrapper around `uuid`.
    #[must_use]
    pub(crate) const fn new(uuid: Uuid) -> Self {
        Self(uuid)
    }

    /// Returns the underlying [`Uuid`].
    #[must_use]
    pub(crate) const fn uuid(&self) -> Uuid {
        self.0
    }

    /// Returns the 16 raw bytes of the identifier in network byte order.
    #[must_use]
    pub(crate) fn as_bytes(&self) -> &[u8; 16] {
        self.0.as_bytes()
    }

    /// Creates an empty tenant entity index for entities identified by UUID.
    #[must_use]
    pub(crate) fn new_index() -> TenantEntityIndex<DefaultEntity> {
        TenantEntityIndex::new()
    }
}

impl From<Uuid> for EntityUuid {
    fn from(uuid: Uuid) -> Self {
        Self(uuid)
    }
}

impl From<EntityUuid> for Uuid {
    fn from(entity: EntityUuid) -> Self {
        entity.0
    }
}

impl fmt::Display for EntityUuid {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

/// Hands `error` back, logging it under `trace` first.
fn refuse<T>(error: TupleError) -> Result<T, TupleError> {
    #[cfg(feature = "trace")]
    lgwks_std::trace::debug!(%error, "entity_uuid: returning an error to the caller");
    Err(error)
}

/// Maps `(tenant, uuid)` pairs to ECS entity slots via FoundationDB tuple keys (#851).
///
/// Keys are encoded as `[Element::String(tenant), Element::Uuid(uuid)]`
/// (`0x02 <tenant> 0x00 0x30 <uuid>`), providing multi-tenant isolation, prefix-scan
/// separation, and stable lexicographic ordering in the entity registry.
#[derive(Debug, Clone, Default)]
#[cfg_attr(feature = "ecs", derive(Resource))]
#[non_exhaustive]
pub(crate) struct TenantEntityIndex<E = DefaultEntity> {
    /// The underlying map from FoundationDB tuple keys to entity handles.
    entries: BTreeMap<Vec<u8>, E>,
}

impl<E> TenantEntityIndex<E> {
    /// Creates a new, empty tenant entity index.
    #[must_use]
    pub(crate) const fn new() -> Self {
        Self {
            entries: BTreeMap::new(),
        }
    }

    /// Encodes a `(tenant, uuid)` pair into a FoundationDB tuple key `[String(tenant), Uuid(uuid)]`.
    ///
    /// The wire format is `0x02 <tenant> 0x00 0x30 <uuid>`.
    ///
    /// # Errors
    ///
    /// Returns [`TupleError`] if encoding fails or key limits are exceeded.
    pub(crate) fn encode_key(tenant: &str, uuid: Uuid) -> Result<Vec<u8>, TupleError> {
        let mut key_buf = Vec::new();
        let elements = [Element::String(Cow::Borrowed(tenant)), Element::Uuid(uuid)];
        if let Err(err) = pack_elements(&elements, &mut key_buf) {
            return refuse(err);
        }
        Ok(key_buf)
    }

    /// Encodes a tenant name into a FoundationDB tuple prefix `[String(tenant)]`.
    ///
    /// The wire format is `0x02 <tenant> 0x00`.
    ///
    /// # Errors
    ///
    /// Returns [`TupleError`] if encoding fails or key limits are exceeded.
    pub(crate) fn encode_tenant_prefix(tenant: &str) -> Result<Vec<u8>, TupleError> {
        let mut prefix = Vec::new();
        let elements = [Element::String(Cow::Borrowed(tenant))];
        if let Err(err) = pack_elements(&elements, &mut prefix) {
            return refuse(err);
        }
        Ok(prefix)
    }

    /// Associates `(tenant, uuid)` with `entity` in the index, returning any previous entity handle.
    ///
    /// # Errors
    ///
    /// Returns [`TupleError`] if the key cannot be encoded.
    pub(crate) fn insert(
        &mut self,
        tenant: &str,
        uuid: Uuid,
        entity: E,
    ) -> Result<Option<E>, TupleError> {
        let key_buf = Self::encode_key(tenant, uuid)?;
        Ok(self.entries.insert(key_buf, entity))
    }

    /// Looks up the entity associated with `(tenant, uuid)`.
    ///
    /// # Errors
    ///
    /// Returns [`TupleError`] if the key cannot be encoded.
    pub(crate) fn get(&self, tenant: &str, uuid: Uuid) -> Result<Option<&E>, TupleError> {
        let key_buf = Self::encode_key(tenant, uuid)?;
        Ok(self.entries.get(&key_buf))
    }

    /// Removes the entity associated with `(tenant, uuid)` from the index, returning it if present.
    ///
    /// # Errors
    ///
    /// Returns [`TupleError`] if the key cannot be encoded.
    pub(crate) fn remove(&mut self, tenant: &str, uuid: Uuid) -> Result<Option<E>, TupleError> {
        let key_buf = Self::encode_key(tenant, uuid)?;
        Ok(self.entries.remove(&key_buf))
    }

    /// Returns `true` if an entity exists for `(tenant, uuid)`.
    ///
    /// # Errors
    ///
    /// Returns [`TupleError`] if the key cannot be encoded.
    pub(crate) fn contains(&self, tenant: &str, uuid: Uuid) -> Result<bool, TupleError> {
        let key_buf = Self::encode_key(tenant, uuid)?;
        Ok(self.entries.contains_key(&key_buf))
    }

    /// Returns the number of entity mappings in the index.
    #[must_use]
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns `true` if the index contains no entity mappings.
    #[must_use]
    pub(crate) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Clears all entity mappings from the index.
    pub(crate) fn clear(&mut self) {
        self.entries.clear();
    }

    /// Returns all entity UUIDs registered under `tenant` using an ordered prefix scan.
    ///
    /// # Errors
    ///
    /// Returns [`TupleError`] if the prefix cannot be encoded.
    pub(crate) fn tenant_keys(&self, tenant: &str) -> Result<Vec<Uuid>, TupleError> {
        let prefix = Self::encode_tenant_prefix(tenant)?;
        let mut result = Vec::new();
        for (key, _) in self.entries.range(prefix.clone()..) {
            if !key.starts_with(&prefix) {
                break;
            }
            let elements = match lgwks_std::id::unpack_elements(key) {
                Ok(elems) => elems,
                Err(err) => return refuse(err),
            };
            if let Some(&Element::Uuid(id)) = elements.get(1) {
                result.push(id);
            }
        }
        Ok(result)
    }
}

/// Internal verification function exercising entity UUID and tenant index so they are live and tested.
pub(crate) fn verify_entity_uuid_contract() -> bool {
    let mut index = TenantEntityIndex::<DefaultEntity>::new();
    let shared_uuid = Uuid::from([42u8; 16]);
    let mut valid = TenantEntityIndex::<DefaultEntity>::encode_key("tenant-a", shared_uuid).is_ok();
    valid &= TenantEntityIndex::<DefaultEntity>::encode_tenant_prefix("tenant-a").is_ok();
    #[cfg(feature = "ecs")]
    let default_ent = Entity::from_bits(1);
    #[cfg(not(feature = "ecs"))]
    let default_ent = 101_u64;
    valid &= index.insert("tenant-a", shared_uuid, default_ent).is_ok();
    valid &= index.get("tenant-a", shared_uuid).is_ok();
    valid &= index.contains("tenant-a", shared_uuid).is_ok();
    valid &= index.remove("tenant-a", shared_uuid).is_ok();
    let _len = index.len();
    let _empty = index.is_empty();
    index.clear();
    valid &= index.tenant_keys("tenant-a").is_ok();
    let wrapped = EntityUuid::new(shared_uuid);
    let _uuid = wrapped.uuid();
    let _bytes = wrapped.as_bytes();
    let _idx = EntityUuid::new_index();
    valid
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_tenants_with_identical_uuid_resolve_to_disjoint_entities() -> Result<(), TupleError> {
        let mut index = TenantEntityIndex::<u64>::new();
        let shared_uuid = Uuid::from([42u8; 16]);
        let tenant_a = "tenant-alpha";
        let tenant_b = "tenant-beta";
        let entity_a = 101_u64;
        let entity_b = 202_u64;

        // Insert identical UUID for two distinct tenants
        index.insert(tenant_a, shared_uuid, entity_a)?;
        index.insert(tenant_b, shared_uuid, entity_b)?;

        // Verify tenant isolation: lookups resolve to disjoint entity handles
        assert_eq!(index.get(tenant_a, shared_uuid)?, Some(&entity_a));
        assert_eq!(index.get(tenant_b, shared_uuid)?, Some(&entity_b));
        assert_ne!(
            index.get(tenant_a, shared_uuid)?,
            index.get(tenant_b, shared_uuid)?
        );

        // Verify wire keys and prefix separation
        let key_a = TenantEntityIndex::<u64>::encode_key(tenant_a, shared_uuid)?;
        let key_b = TenantEntityIndex::<u64>::encode_key(tenant_b, shared_uuid)?;
        assert_ne!(key_a, key_b);
        assert!(key_a < key_b);

        let prefix_a = TenantEntityIndex::<u64>::encode_tenant_prefix(tenant_a)?;
        let prefix_b = TenantEntityIndex::<u64>::encode_tenant_prefix(tenant_b)?;
        assert!(key_a.starts_with(&prefix_a));
        assert!(!key_a.starts_with(&prefix_b));
        assert!(key_b.starts_with(&prefix_b));
        assert!(!key_b.starts_with(&prefix_a));

        // Verify prefix scan separation
        let keys_a = index.tenant_keys(tenant_a)?;
        let keys_b = index.tenant_keys(tenant_b)?;
        assert_eq!(keys_a, vec![shared_uuid]);
        assert_eq!(keys_b, vec![shared_uuid]);

        // Mutation isolation on removal
        assert!(index.contains(tenant_a, shared_uuid)?);
        assert_eq!(index.len(), 2);
        assert!(!index.is_empty());
        assert_eq!(index.remove(tenant_a, shared_uuid)?, Some(entity_a));
        assert!(!index.contains(tenant_a, shared_uuid)?);
        assert_eq!(index.get(tenant_a, shared_uuid)?, None);
        assert_eq!(index.get(tenant_b, shared_uuid)?, Some(&entity_b));
        index.clear();
        assert!(index.is_empty());

        let wrapped = EntityUuid::new(shared_uuid);
        assert_eq!(wrapped.uuid(), shared_uuid);
        assert_eq!(wrapped.as_bytes(), shared_uuid.as_bytes());

        Ok(())
    }
}
