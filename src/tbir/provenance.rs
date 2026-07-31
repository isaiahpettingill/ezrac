use crate::{
    ast::{Declaration, Program, Type},
    compat::prelude::*,
};

use super::{TbirAccess, TbirMemoryModel, TbirMemoryObject, TbirObjectKind, model::SemanticModel};

#[derive(Clone, Debug, Default)]
pub struct OptimizationContext {
    pub objects: HashMap<String, TbirMemoryObject>,
}

impl OptimizationContext {
    pub fn from_objects(objects: &[TbirMemoryObject]) -> Self {
        Self {
            objects: objects
                .iter()
                .map(|object| (object.name.clone(), object.clone()))
                .collect(),
        }
    }
}

pub fn memory_objects(
    program: &Program,
    model: &SemanticModel,
    memory: &TbirMemoryModel,
) -> Vec<TbirMemoryObject> {
    let mut objects = Vec::new();
    for (name, storage) in &model.globals {
        let ty = model.global_types[name].clone();
        objects.push(object(
            name,
            TbirObjectKind::Global,
            ty,
            storage.address,
            storage.size,
            TbirAccess::ReadWrite,
            false,
            memory,
        ));
    }
    for (name, (address, ty, volatile)) in &model.mmio {
        let size = match ty {
            Type::Ptr(pointee) => model
                .type_size(pointee)
                .unwrap_or_else(|_| u32::from(model.pointer_bytes())),
            _ => u32::from(model.pointer_bytes()),
        };
        objects.push(object(
            name,
            TbirObjectKind::Mmio,
            ty.clone(),
            *address,
            size,
            TbirAccess::ReadWrite,
            *volatile,
            memory,
        ));
    }
    for (name, embed) in &model.embeds {
        objects.push(object(
            name,
            TbirObjectKind::Embed,
            embed.ty.clone(),
            embed.storage.address,
            embed.storage.size,
            TbirAccess::ReadOnly,
            false,
            memory,
        ));
    }

    // Preserve declaration order so dumps and tests are stable.
    let order: HashMap<String, usize> = program
        .declarations
        .iter()
        .enumerate()
        .filter_map(|(index, declaration)| match declaration {
            Declaration::Global(value) => Some((value.name.clone(), index)),
            Declaration::Mmio(value) => Some((value.name.clone(), index)),
            Declaration::Embed(value) => Some((value.name.clone(), index)),
            _ => None,
        })
        .collect();
    objects.sort_by_key(|object| order.get(&object.name).copied().unwrap_or(usize::MAX));
    objects
}

#[allow(clippy::too_many_arguments)]
fn object(
    name: &str,
    kind: TbirObjectKind,
    ty: Type,
    address: u32,
    size: u32,
    access: TbirAccess,
    volatile: bool,
    memory: &TbirMemoryModel,
) -> TbirMemoryObject {
    let end = address.checked_add(size);
    let region = memory.regions.iter().find(|region| {
        let region_end = region.start.checked_add(region.size);
        end.is_some_and(|end| {
            region_end.is_some_and(|region_end| address >= region.start && end <= region_end)
        })
    });
    TbirMemoryObject {
        name: name.to_owned(),
        kind,
        ty,
        address,
        size,
        region: region.map(|region| region.name.clone()),
        access: match (kind, region.map(|region| region.access)) {
            (TbirObjectKind::Mmio, _) => access,
            (_, Some(TbirAccess::ReadOnly)) => TbirAccess::ReadOnly,
            _ => access,
        },
        volatile: volatile || region.is_some_and(|region| region.volatile),
    }
}
