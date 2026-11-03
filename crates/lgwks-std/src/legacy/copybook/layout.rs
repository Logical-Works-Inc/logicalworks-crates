//! The copybook level tree: building, expanding and laying out.
//!
//! [`build_tree`] nests flat items by level under the nesting ceiling;
//! [`expand_tree`] walks the tree into laid-out [`LayoutField`] items with
//! byte offsets, expanding fixed `OCCURS` inline, overlaying `REDEFINES`
//! and keeping one template for each dynamic group.
//!
//! [`LayoutField`]: super::model::LayoutField

use super::super::decimal::packed_width;
use super::super::limits::Limits;
use super::model::{CopybookError, LayoutField};
use super::{
    Item, NameBinding, Node, Resolution, Slot, SlotKind, binding_is_numeric, resolve_name,
};

/// Builds the level tree from flat items, enforcing the nesting ceiling.
pub(crate) fn build_tree(items: Vec<Item>, limits: &Limits) -> Result<Vec<Node>, CopybookError> {
    let mut roots: Vec<Node> = Vec::new();
    let mut stack: Vec<Node> = Vec::new();
    for item in items.into_iter() {
        limits.check_nesting(stack.len().saturating_add(1))?;
        while stack
            .last()
            .is_some_and(|top: &Node| top.item.level >= item.level)
        {
            let Some(done) = stack.pop() else {
                break;
            };
            if let Some(parent) = stack.last_mut() {
                parent.children.push(done);
            } else {
                roots.push(done);
            }
        }
        stack.push(Node {
            item,
            children: Vec::new(),
        });
    }
    while let Some(done) = stack.pop() {
        if let Some(parent) = stack.last_mut() {
            parent.children.push(done);
        } else {
            roots.push(done);
        }
    }
    Ok(roots)
}

/// The byte length of one picture, without repetition.
fn picture_len(picture: &super::Picture) -> usize {
    match picture.text_len {
        Some(len) => len,
        None => match picture.usage {
            super::Usage::Display => picture.digits,
            super::Usage::Packed => packed_width(picture.digits),
            super::Usage::Binary => binary_width(picture.digits),
        },
    }
}

/// The byte width of a binary picture: 2, 4 or 8 by its digits.
pub(crate) const fn binary_width(digits: usize) -> usize {
    if digits <= 4 {
        2
    } else if digits <= 9 {
        4
    } else {
        8
    }
}

/// The slot kind of one picture.
fn slot_kind(picture: &super::Picture) -> SlotKind {
    match picture.text_len {
        Some(_) => SlotKind::Text,
        None => match picture.usage {
            super::Usage::Display => SlotKind::Zoned {
                digits: picture.digits,
                signed: picture.signed,
            },
            super::Usage::Packed => SlotKind::Packed {
                digits: picture.digits,
                signed: picture.signed,
            },
            super::Usage::Binary => SlotKind::Binary {
                digits: picture.digits,
                signed: picture.signed,
            },
        },
    }
}

/// Expands the level tree into laid-out fields with offsets.
pub(crate) fn expand_tree(
    roots: Vec<Node>,
    limits: &Limits,
) -> Result<super::RecordLayout, CopybookError> {
    let mut expander = Expander::new(limits);
    let mut cursor: usize = 0;
    for root in roots.iter() {
        cursor = expander.node(root, cursor, 1)?;
    }
    limits.check_bytes(cursor)?;
    limits.check_records(expander.fields.len())?;
    Ok(super::RecordLayout::from_parts(expander.fields, cursor))
}

/// Reads the repetition of one item: 1, a fixed count, or a depending range.
fn occurs_of(item: &Item) -> (usize, usize, Option<String>) {
    match item.occurs.as_ref() {
        Some(occurs) => (occurs.min, occurs.max, occurs.depending_on.clone()),
        None => (1, 1, None),
    }
}

/// A `PIC` diagnostic naming the node's line and field.
fn bad_pic(node: &Node, text: &str) -> CopybookError {
    CopybookError::BadPic {
        line: node.item.line_no,
        field: node.item.name.clone(),
        text: String::from(text),
    }
}

/// An `OCCURS` diagnostic naming the node's line and field.
fn bad_occurs(node: &Node, text: &str) -> CopybookError {
    CopybookError::BadOccurs {
        line: node.item.line_no,
        field: node.item.name.clone(),
        text: String::from(text),
    }
}

/// One expansion pass: the fields laid out so far, the names bound so far,
/// and the bounds every step is checked against.
struct Expander<'limits> {
    /// Laid-out fields in declaration order.
    fields: Vec<LayoutField>,
    /// Names bound so far, for `REDEFINES` and `DEPENDING ON`.
    bindings: Vec<NameBinding>,
    /// The ceilings every step enforces.
    limits: &'limits Limits,
}

impl<'limits> Expander<'limits> {
    /// An empty pass under `limits`.
    const fn new(limits: &'limits Limits) -> Self {
        Self {
            fields: Vec::new(),
            bindings: Vec::new(),
            limits,
        }
    }

    /// Binds the node's name over `len` bytes at `offset`.
    fn bind(&mut self, node: &Node, offset: usize, len: usize, numeric: bool) {
        self.bindings.push(NameBinding {
            name: node.item.name.clone(),
            offset,
            len,
            numeric,
        });
    }

    /// Pushes one field, refusing past the record ceiling.
    fn push(&mut self, field: LayoutField) -> Result<(), CopybookError> {
        self.limits
            .check_records(self.fields.len().saturating_add(1))?;
        self.fields.push(field);
        Ok(())
    }

    /// Expands one node at `cursor`, returning the cursor past it.
    fn node(&mut self, node: &Node, cursor: usize, depth: usize) -> Result<usize, CopybookError> {
        self.limits.check_nesting(depth)?;
        let base = match node.item.redefines.as_ref() {
            Some(target) => self.redefined_base(node, target)?,
            None => cursor,
        };
        if node.children.is_empty() {
            let end = self.leaf(node, base)?;
            return Ok(cursor.max(end));
        }
        if node.item.picture.is_some() {
            let refusal = Err(bad_pic(node, "<group with picture>"));
            #[cfg(feature = "trace")]
            crate::trace::debug!("copybook: refusing a group carrying a picture");
            return refusal;
        }
        let (_, max, depending_on) = occurs_of(&node.item);
        if depending_on.is_some() {
            let end = self.dynamic_group(node, base, depth)?;
            return Ok(cursor.max(end));
        }
        let mut end = base;
        for _ in 0..max {
            for child in node.children.iter() {
                end = self.node(child, end, depth.saturating_add(1))?;
            }
        }
        self.bind(node, base, end.saturating_sub(base), false);
        Ok(cursor.max(end))
    }

    /// The offset a `REDEFINES` overlays: its one earlier target.
    fn redefined_base(&self, node: &Node, target: &str) -> Result<usize, CopybookError> {
        match resolve_name(&self.bindings, target) {
            Resolution::One(offset, _) => Ok(offset),
            Resolution::Missing => {
                let refusal = Err(CopybookError::UnknownRedefines {
                    line: node.item.line_no,
                    field: node.item.name.clone(),
                    name: String::from(target),
                });
                #[cfg(feature = "trace")]
                crate::trace::debug!("copybook: refusing a REDEFINES past a missing target");
                refusal
            }
            Resolution::Ambiguous => {
                let refusal = Err(CopybookError::AmbiguousName {
                    line: node.item.line_no,
                    name: String::from(target),
                });
                #[cfg(feature = "trace")]
                crate::trace::debug!("copybook: refusing a REDEFINES past an ambiguous target");
                refusal
            }
        }
    }

    /// Expands one elementary item at `base`, returning the offset past it.
    fn leaf(&mut self, node: &Node, base: usize) -> Result<usize, CopybookError> {
        let Some(picture) = node.item.picture else {
            let refusal = Err(bad_pic(node, "<no picture>"));
            #[cfg(feature = "trace")]
            crate::trace::debug!("copybook: refusing an elementary item with no picture");
            return refusal;
        };
        let stride = picture_len(&picture);
        let (min, max, depending_on) = occurs_of(&node.item);
        if let Some(holder) = depending_on.as_ref() {
            check_holder(node, holder, &self.bindings)?;
        }
        let total = max.saturating_mul(stride);
        self.limits.check_bytes(base.saturating_add(total))?;
        let kind = slot_kind(&picture);
        let numeric = !matches!(kind, SlotKind::Text);
        let mut slot = Slot {
            name: node.item.name.clone(),
            offset: base,
            stride,
            len: stride,
            kind,
            scale: picture.scale,
            min_occurs: 1,
            max_occurs: 1,
            depending_on: None,
        };
        if depending_on.is_none() {
            for occurrence in 0..max {
                slot.offset = base.saturating_add(occurrence.saturating_mul(stride));
                self.push(LayoutField::from_slot(&slot, Vec::new()))?;
            }
        } else {
            slot.len = total;
            slot.min_occurs = min;
            slot.max_occurs = max;
            slot.depending_on = depending_on;
            self.push(LayoutField::from_slot(&slot, Vec::new()))?;
        }
        self.bind(node, base, total, numeric);
        Ok(base.saturating_add(total))
    }

    /// Expands a dynamic group: one template occurrence laid out, the count
    /// read from its `DEPENDING ON` field at decode time.
    fn dynamic_group(
        &mut self,
        node: &Node,
        base: usize,
        depth: usize,
    ) -> Result<usize, CopybookError> {
        let (min, max, holder) = occurs_of(&node.item);
        let Some(holder) = holder else {
            let refusal = Err(bad_occurs(node, "<dynamic group with no holder>"));
            #[cfg(feature = "trace")]
            crate::trace::debug!("copybook: refusing a dynamic group with no holder");
            return refusal;
        };
        check_holder(node, &holder, &self.bindings)?;
        let mut template = Expander::new(self.limits);
        let mut end = base;
        for child in node.children.iter() {
            end = template.template(child, end, depth)?;
        }
        let stride = end.saturating_sub(base);
        let total = max.saturating_mul(stride);
        self.limits.check_bytes(base.saturating_add(total))?;
        let group = Slot {
            name: node.item.name.clone(),
            offset: base,
            stride,
            len: total,
            kind: SlotKind::Text,
            scale: 0,
            min_occurs: min,
            max_occurs: max,
            depending_on: Some(holder),
        };
        self.push(LayoutField::from_slot(&group, template.fields))?;
        self.bind(node, base, total, false);
        Ok(base.saturating_add(total))
    }

    /// Expands one template child at `base` for a dynamic group.
    fn template(&mut self, node: &Node, base: usize, depth: usize) -> Result<usize, CopybookError> {
        self.limits.check_nesting(depth.saturating_add(1))?;
        if node.item.redefines.is_some() {
            let refusal = Err(CopybookError::UnknownClause {
                line: node.item.line_no,
                text: String::from("REDEFINES inside a dynamic group"),
            });
            #[cfg(feature = "trace")]
            crate::trace::debug!("copybook: refusing a REDEFINES inside a dynamic group");
            return refusal;
        }
        if node.children.is_empty() {
            let Some(picture) = node.item.picture else {
                let refusal = Err(bad_pic(node, "<no picture>"));
                #[cfg(feature = "trace")]
                crate::trace::debug!("copybook: refusing a template leaf with no picture");
                return refusal;
            };
            if node.item.occurs.is_some() {
                let refusal = Err(bad_occurs(node, "OCCURS inside a dynamic group"));
                #[cfg(feature = "trace")]
                crate::trace::debug!("copybook: refusing a nested OCCURS inside a dynamic group");
                return refusal;
            }
            let stride = picture_len(&picture);
            self.push(LayoutField::template_leaf(
                node.item.name.clone(),
                base,
                stride,
                slot_kind(&picture),
                picture.scale,
            ))?;
            self.bind(node, base, stride, true);
            return Ok(base.saturating_add(stride));
        }
        let mut end = base;
        for child in node.children.iter() {
            end = self.template(child, end, depth.saturating_add(1))?;
        }
        self.bind(node, base, end.saturating_sub(base), false);
        Ok(end)
    }
}

/// Checks a `DEPENDING ON` holder names one earlier numeric field.
fn check_holder(node: &Node, holder: &str, bindings: &[NameBinding]) -> Result<(), CopybookError> {
    match resolve_name(bindings, holder) {
        Resolution::One(_, _) => {}
        Resolution::Missing => {
            let refusal = Err(CopybookError::DependsOnMissing {
                line: node.item.line_no,
                field: node.item.name.clone(),
                name: String::from(holder),
            });
            #[cfg(feature = "trace")]
            crate::trace::debug!("copybook: refusing a DEPENDING ON past a missing field");
            return refusal;
        }
        Resolution::Ambiguous => {
            let refusal = Err(CopybookError::AmbiguousName {
                line: node.item.line_no,
                name: String::from(holder),
            });
            #[cfg(feature = "trace")]
            crate::trace::debug!("copybook: refusing a DEPENDING ON past an ambiguous field");
            return refusal;
        }
    }
    if !binding_is_numeric(bindings, holder) {
        let refusal = Err(CopybookError::DependsOnNotNumeric {
            line: node.item.line_no,
            field: node.item.name.clone(),
            name: String::from(holder),
        });
        #[cfg(feature = "trace")]
        crate::trace::debug!("copybook: refusing a DEPENDING ON past a non-numeric field");
        return refusal;
    }
    Ok(())
}
