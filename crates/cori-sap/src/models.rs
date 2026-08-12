use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

use crate::error::{AdapterError, Result};

/// Stable, provider-neutral purchase-order header emitted by the adapter.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PurchaseOrder {
    pub purchase_order: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub purchase_order_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub company_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub purchasing_organization: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub purchasing_group: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supplier: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub document_currency: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub purchase_order_date: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub creation_date: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_change_date_time: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub processing_status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payment_terms: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deletion_code: Option<String>,
}

/// Stable, provider-neutral purchase-order item emitted by the adapter.
/// Decimal values remain strings so normalization never loses SAP precision.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PurchaseOrderItem {
    pub purchase_order: String,
    pub purchase_order_item: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub material: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub material_group: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plant: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub storage_location: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub order_quantity: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub order_unit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub net_price_amount: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub net_price_quantity: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub price_unit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub document_currency: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item_category: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_assignment_category: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completely_delivered: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finally_invoiced: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deletion_code: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PurchaseOrderGetOutput {
    pub purchase_order: PurchaseOrder,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PurchaseOrderListOutput {
    pub purchase_orders: Vec<PurchaseOrder>,
    pub count: usize,
    pub limit: usize,
    pub has_more: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PurchaseOrderItemsOutput {
    pub purchase_order: String,
    pub items: Vec<PurchaseOrderItem>,
    pub count: usize,
    pub limit: usize,
    pub has_more: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct SapPurchaseOrder {
    #[serde(default)]
    purchase_order: Option<String>,
    #[serde(default)]
    purchase_order_type: Option<String>,
    #[serde(default)]
    company_code: Option<String>,
    #[serde(default)]
    purchasing_organization: Option<String>,
    #[serde(default)]
    purchasing_group: Option<String>,
    #[serde(default)]
    supplier: Option<String>,
    #[serde(default)]
    document_currency: Option<String>,
    #[serde(default)]
    purchase_order_date: Option<String>,
    #[serde(default)]
    creation_date: Option<String>,
    #[serde(default)]
    last_change_date_time: Option<String>,
    #[serde(default)]
    purchasing_processing_status: Option<String>,
    #[serde(default)]
    language: Option<String>,
    #[serde(default)]
    payment_terms: Option<String>,
    #[serde(default)]
    purchase_order_deletion_code: Option<String>,
}

impl TryFrom<SapPurchaseOrder> for PurchaseOrder {
    type Error = AdapterError;

    fn try_from(value: SapPurchaseOrder) -> Result<Self> {
        Ok(Self {
            purchase_order: required(value.purchase_order, "PurchaseOrder")?,
            purchase_order_type: clean(value.purchase_order_type),
            company_code: clean(value.company_code),
            purchasing_organization: clean(value.purchasing_organization),
            purchasing_group: clean(value.purchasing_group),
            supplier: clean(value.supplier),
            document_currency: clean(value.document_currency),
            purchase_order_date: clean(value.purchase_order_date),
            creation_date: clean(value.creation_date),
            last_change_date_time: clean(value.last_change_date_time),
            processing_status: clean(value.purchasing_processing_status),
            language: clean(value.language),
            payment_terms: clean(value.payment_terms),
            deletion_code: clean(value.purchase_order_deletion_code),
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct SapPurchaseOrderItem {
    #[serde(default)]
    purchase_order: Option<String>,
    #[serde(default)]
    purchase_order_item: Option<String>,
    #[serde(default)]
    purchase_order_item_text: Option<String>,
    #[serde(default)]
    material: Option<String>,
    #[serde(default)]
    material_group: Option<String>,
    #[serde(default)]
    plant: Option<String>,
    #[serde(default)]
    storage_location: Option<String>,
    #[serde(default)]
    order_quantity: Option<JsonValue>,
    #[serde(default)]
    purchase_order_quantity_unit: Option<String>,
    #[serde(default)]
    net_price_amount: Option<JsonValue>,
    #[serde(default)]
    net_price_quantity: Option<JsonValue>,
    #[serde(default)]
    order_price_unit: Option<String>,
    #[serde(default)]
    document_currency: Option<String>,
    #[serde(default)]
    purchase_order_item_category: Option<String>,
    #[serde(default)]
    account_assignment_category: Option<String>,
    #[serde(default)]
    is_completely_delivered: Option<bool>,
    #[serde(default)]
    is_finally_invoiced: Option<bool>,
    #[serde(default)]
    purchasing_document_deletion_code: Option<String>,
}

impl TryFrom<SapPurchaseOrderItem> for PurchaseOrderItem {
    type Error = AdapterError;

    fn try_from(value: SapPurchaseOrderItem) -> Result<Self> {
        Ok(Self {
            purchase_order: required(value.purchase_order, "PurchaseOrder")?,
            purchase_order_item: required(value.purchase_order_item, "PurchaseOrderItem")?,
            text: clean(value.purchase_order_item_text),
            material: clean(value.material),
            material_group: clean(value.material_group),
            plant: clean(value.plant),
            storage_location: clean(value.storage_location),
            order_quantity: scalar_string(value.order_quantity)?,
            order_unit: clean(value.purchase_order_quantity_unit),
            net_price_amount: scalar_string(value.net_price_amount)?,
            net_price_quantity: scalar_string(value.net_price_quantity)?,
            price_unit: clean(value.order_price_unit),
            document_currency: clean(value.document_currency),
            item_category: clean(value.purchase_order_item_category),
            account_assignment_category: clean(value.account_assignment_category),
            completely_delivered: value.is_completely_delivered,
            finally_invoiced: value.is_finally_invoiced,
            deletion_code: clean(value.purchasing_document_deletion_code),
        })
    }
}

fn required(value: Option<String>, field: &'static str) -> Result<String> {
    clean(value).ok_or(AdapterError::InvalidResponse {
        reason: match field {
            "PurchaseOrder" => "record is missing PurchaseOrder",
            "PurchaseOrderItem" => "record is missing PurchaseOrderItem",
            _ => "record is missing a required key",
        },
    })
}

fn clean(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        let trimmed = value.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    })
}

fn scalar_string(value: Option<JsonValue>) -> Result<Option<String>> {
    match value {
        None | Some(JsonValue::Null) => Ok(None),
        Some(JsonValue::String(value)) => Ok(clean(Some(value))),
        Some(JsonValue::Number(value)) => Ok(Some(value.to_string())),
        Some(_) => Err(AdapterError::InvalidResponse {
            reason: "numeric field was not a JSON number or string",
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_decimal_numbers_without_losing_precision() {
        let raw: SapPurchaseOrderItem = serde_json::from_value(serde_json::json!({
            "PurchaseOrder": "4500001234",
            "PurchaseOrderItem": "00010",
            "OrderQuantity": 7.25,
            "NetPriceAmount": "130.00",
            "NetPriceQuantity": "1",
            "OrderPriceUnit": "CRT",
            "IsCompletelyDelivered": false
        }))
        .expect("SAP item");
        let item = PurchaseOrderItem::try_from(raw).expect("normalized item");
        assert_eq!(item.order_quantity.as_deref(), Some("7.25"));
        assert_eq!(item.net_price_amount.as_deref(), Some("130.00"));
        assert_eq!(item.net_price_quantity.as_deref(), Some("1"));
        assert_eq!(item.price_unit.as_deref(), Some("CRT"));
        assert_eq!(item.completely_delivered, Some(false));
    }

    #[test]
    fn required_keys_are_enforced() {
        let raw: SapPurchaseOrder =
            serde_json::from_value(serde_json::json!({"Supplier": "1000"})).expect("SAP header");
        assert!(matches!(
            PurchaseOrder::try_from(raw),
            Err(AdapterError::InvalidResponse { .. })
        ));
    }
}
