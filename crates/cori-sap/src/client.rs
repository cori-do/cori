use std::collections::HashSet;
use std::io::Read as _;
use std::time::{Duration, Instant};

use reqwest::blocking::{Client, Response};
use reqwest::header::{ACCEPT, CONTENT_LENGTH};
use reqwest::redirect::Policy;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use url::Url;

use crate::config::MachineProfile;
use crate::error::{AdapterError, Result};
use crate::models::{
    PurchaseOrder, PurchaseOrderGetOutput, PurchaseOrderItem, PurchaseOrderItemsOutput,
    PurchaseOrderListOutput, SapPurchaseOrder, SapPurchaseOrderItem,
};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
// Cori's reference SAP activity has a 60-second start-to-close timeout. Keep
// the entire paginated HTTP operation below that boundary so the adapter can
// serialize its result and exit before Temporal times out the activity.
const OPERATION_TIMEOUT: Duration = Duration::from_secs(50);
const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
const MAX_PAGES: usize = 20;
const ACCEPT_JSON: &str = "application/json;IEEE754Compatible=true";

const PURCHASE_ORDER_ENTITY: &str = "PurchaseOrder";
const PURCHASE_ORDER_ITEM_ENTITY: &str = "PurchaseOrderItem";

const PURCHASE_ORDER_SELECT: &str = concat!(
    "PurchaseOrder,PurchaseOrderType,CompanyCode,PurchasingOrganization,",
    "PurchasingGroup,Supplier,DocumentCurrency,PurchaseOrderDate,CreationDate,",
    "LastChangeDateTime,PurchasingProcessingStatus,Language,",
    "PaymentTerms,PurchaseOrderDeletionCode"
);

const PURCHASE_ORDER_ITEM_SELECT: &str = concat!(
    "PurchaseOrder,PurchaseOrderItem,PurchaseOrderItemText,Material,MaterialGroup,",
    "Plant,StorageLocation,OrderQuantity,PurchaseOrderQuantityUnit,NetPriceAmount,",
    "NetPriceQuantity,OrderPriceUnit,DocumentCurrency,PurchaseOrderItemCategory,",
    "AccountAssignmentCategory,IsCompletelyDelivered,IsFinallyInvoiced,",
    "PurchasingDocumentDeletionCode"
);

/// HTTPS/OData client bound to one validated machine profile.
pub struct SapClient {
    http: Client,
    profile: MachineProfile,
    bearer_token: String,
}

impl SapClient {
    pub fn new(profile: MachineProfile, bearer_token: String) -> Result<Self> {
        let http = Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .redirect(Policy::none())
            .user_agent(concat!("cori-sap/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|_| AdapterError::Transport)?;
        Ok(Self {
            http,
            profile,
            bearer_token,
        })
    }

    pub fn get_purchase_order(&self, id: &str) -> Result<PurchaseOrderGetOutput> {
        let id = crate::validate_purchase_order_id(id)?;
        let deadline = OperationDeadline::start();
        let filter = format!("PurchaseOrder eq '{id}'");
        let collected: Collected<SapPurchaseOrder> = self.collect(
            PURCHASE_ORDER_ENTITY,
            PURCHASE_ORDER_SELECT,
            Some(&filter),
            None,
            2,
            &deadline,
        )?;
        let mut records = collected.records;
        if records.is_empty() {
            return Err(AdapterError::PurchaseOrderNotFound { id: id.to_string() });
        }
        if records.len() != 1 || collected.has_more {
            return Err(AdapterError::InvalidResponse {
                reason: "purchase-order key returned more than one record",
            });
        }
        let record = records.pop().ok_or(AdapterError::InvalidResponse {
            reason: "purchase-order response unexpectedly became empty",
        })?;
        let purchase_order = PurchaseOrder::try_from(record)?;
        if purchase_order.purchase_order != id {
            return Err(AdapterError::InvalidResponse {
                reason: "purchase-order response key did not match the request",
            });
        }
        Ok(PurchaseOrderGetOutput { purchase_order })
    }

    pub fn list_purchase_orders(&self, limit: usize) -> Result<PurchaseOrderListOutput> {
        let limit = crate::validate_record_limit(limit)?;
        let deadline = OperationDeadline::start();
        let fetch_limit = limit + 1;
        let collected: Collected<SapPurchaseOrder> = self.collect(
            PURCHASE_ORDER_ENTITY,
            PURCHASE_ORDER_SELECT,
            None,
            Some("PurchaseOrder desc"),
            fetch_limit,
            &deadline,
        )?;
        let mut purchase_orders = collected
            .records
            .into_iter()
            .map(PurchaseOrder::try_from)
            .collect::<Result<Vec<_>>>()?;
        let has_more = collected.has_more || purchase_orders.len() > limit;
        purchase_orders.truncate(limit);
        let count = purchase_orders.len();
        Ok(PurchaseOrderListOutput {
            purchase_orders,
            count,
            limit,
            has_more,
        })
    }

    pub fn list_purchase_order_items(
        &self,
        id: &str,
        limit: usize,
    ) -> Result<PurchaseOrderItemsOutput> {
        let id = crate::validate_purchase_order_id(id)?;
        let limit = crate::validate_record_limit(limit)?;
        let deadline = OperationDeadline::start();
        let fetch_limit = limit + 1;
        let filter = format!("PurchaseOrder eq '{id}'");
        let collected: Collected<SapPurchaseOrderItem> = self.collect(
            PURCHASE_ORDER_ITEM_ENTITY,
            PURCHASE_ORDER_ITEM_SELECT,
            Some(&filter),
            Some("PurchaseOrderItem asc"),
            fetch_limit,
            &deadline,
        )?;
        let mut items = collected
            .records
            .into_iter()
            .map(PurchaseOrderItem::try_from)
            .collect::<Result<Vec<_>>>()?;
        if items.iter().any(|item| item.purchase_order != id) {
            return Err(AdapterError::InvalidResponse {
                reason: "purchase-order item response key did not match the request",
            });
        }
        let has_more = collected.has_more || items.len() > limit;
        items.truncate(limit);
        let count = items.len();
        Ok(PurchaseOrderItemsOutput {
            purchase_order: id.to_string(),
            items,
            count,
            limit,
            has_more,
        })
    }

    fn collect<T>(
        &self,
        entity: &'static str,
        select: &'static str,
        filter: Option<&str>,
        order_by: Option<&str>,
        limit: usize,
        deadline: &OperationDeadline,
    ) -> Result<Collected<T>>
    where
        T: DeserializeOwned,
    {
        let mut records = Vec::with_capacity(limit);
        let mut cursor: Option<PageCursor> = None;
        let mut seen_cursors = HashSet::new();
        let mut pages = 0_usize;

        loop {
            if pages >= MAX_PAGES {
                return Err(AdapterError::PaginationLimit);
            }
            pages += 1;
            let remaining = limit.saturating_sub(records.len());
            if remaining == 0 {
                return Ok(Collected {
                    records,
                    has_more: cursor.is_some(),
                });
            }

            let url =
                self.collection_url(entity, select, filter, order_by, remaining, cursor.as_ref())?;
            let page: ODataPage<T> = self.fetch_page(url, deadline)?;
            let page_had_extra = page.value.len() > remaining;
            records.extend(page.value.into_iter().take(remaining));

            let next_cursor = match page.next_link {
                Some(link) => Some(self.parse_next_cursor(entity, &link)?),
                None => None,
            };
            if records.len() == limit {
                return Ok(Collected {
                    records,
                    has_more: page_had_extra || next_cursor.is_some(),
                });
            }
            let Some(next_cursor) = next_cursor else {
                return Ok(Collected {
                    records,
                    has_more: page_had_extra,
                });
            };
            if !seen_cursors.insert(next_cursor.identity()) {
                return Err(AdapterError::InvalidResponse {
                    reason: "SAP returned a repeated pagination cursor",
                });
            }
            cursor = Some(next_cursor);
        }
    }

    fn collection_url(
        &self,
        entity: &str,
        select: &str,
        filter: Option<&str>,
        order_by: Option<&str>,
        top: usize,
        cursor: Option<&PageCursor>,
    ) -> Result<Url> {
        let mut url = self.profile.service_root().join(entity).map_err(|_| {
            AdapterError::InvalidEndpoint {
                reason: "fixed SAP entity path could not be constructed",
            }
        })?;
        {
            let mut query = url.query_pairs_mut();
            query.append_pair("$select", select);
            if let Some(filter) = filter {
                query.append_pair("$filter", filter);
            }
            if let Some(order_by) = order_by {
                query.append_pair("$orderby", order_by);
            }
            query.append_pair("$top", &top.to_string());
            if let Some(sap_client) = self.profile.sap_client() {
                query.append_pair("sap-client", sap_client);
            }
            if let Some(cursor) = cursor {
                match cursor {
                    PageCursor::SkipToken(token) => {
                        query.append_pair("$skiptoken", token);
                    }
                    PageCursor::Skip(skip) => {
                        query.append_pair("$skip", &skip.to_string());
                    }
                }
            }
        }
        Ok(url)
    }

    fn fetch_page<T: DeserializeOwned>(
        &self,
        url: Url,
        deadline: &OperationDeadline,
    ) -> Result<ODataPage<T>> {
        let request_timeout = deadline.request_timeout()?;
        let response = self
            .http
            .get(url)
            .timeout(request_timeout)
            .header(ACCEPT, ACCEPT_JSON)
            .bearer_auth(&self.bearer_token)
            .send()
            .map_err(|_| AdapterError::Transport)?;
        let status = response.status();
        let body = read_bounded_body(response)?;
        deadline.ensure_remaining()?;
        if !status.is_success() {
            return Err(AdapterError::HttpStatus {
                status: status.as_u16(),
                sap_code: extract_safe_sap_code(&body),
            });
        }
        serde_json::from_slice(&body).map_err(|_| AdapterError::InvalidResponse {
            reason: "body was not a valid OData V4 collection",
        })
    }

    /// Accept only an opaque pagination cursor from a same-origin, same-entity
    /// nextLink. Every other query component is rebuilt from fixed adapter
    /// constants for the next request.
    fn parse_next_cursor(&self, entity: &str, next_link: &str) -> Result<PageCursor> {
        if next_link.len() > 8 * 1024 || next_link.contains(['\r', '\n']) {
            return Err(AdapterError::InvalidResponse {
                reason: "pagination link was invalid",
            });
        }
        let url = self.profile.service_root().join(next_link).map_err(|_| {
            AdapterError::InvalidResponse {
                reason: "pagination link was not a valid URL",
            }
        })?;
        if !same_origin(self.profile.service_root(), &url) {
            return Err(AdapterError::InvalidResponse {
                reason: "pagination link changed SAP origin",
            });
        }
        let expected_path = format!("{}{entity}", self.profile.service_root().path());
        if url.path() != expected_path {
            return Err(AdapterError::InvalidResponse {
                reason: "pagination link changed SAP entity",
            });
        }

        let mut cursor = None;
        for (key, value) in url.query_pairs() {
            match key.as_ref() {
                "$skiptoken" => {
                    if cursor.is_some() || value.is_empty() || value.len() > 4096 {
                        return Err(AdapterError::InvalidResponse {
                            reason: "pagination link had an invalid cursor",
                        });
                    }
                    cursor = Some(PageCursor::SkipToken(value.into_owned()));
                }
                "$skip" => {
                    if cursor.is_some() {
                        return Err(AdapterError::InvalidResponse {
                            reason: "pagination link had multiple cursors",
                        });
                    }
                    let skip =
                        value
                            .parse::<usize>()
                            .map_err(|_| AdapterError::InvalidResponse {
                                reason: "pagination link had an invalid skip value",
                            })?;
                    cursor = Some(PageCursor::Skip(skip));
                }
                "$select" | "$filter" | "$orderby" | "$top" | "sap-client" => {}
                _ => {
                    return Err(AdapterError::InvalidResponse {
                        reason: "pagination link introduced an unsupported query field",
                    });
                }
            }
        }
        cursor.ok_or(AdapterError::InvalidResponse {
            reason: "pagination link did not contain a cursor",
        })
    }

    #[cfg(test)]
    fn for_loopback_test(origin: &str) -> Result<Self> {
        Self::new(
            MachineProfile::for_loopback_test(origin)?,
            "test-token".to_string(),
        )
    }
}

/// One monotonic time budget shared by every HTTP page in an adapter command.
/// Exhaustion is a transient transport failure, matching request timeouts.
#[derive(Debug, Clone, Copy)]
struct OperationDeadline {
    expires_at: Instant,
}

impl OperationDeadline {
    fn start() -> Self {
        Self::from_start(Instant::now())
    }

    fn from_start(started_at: Instant) -> Self {
        Self {
            expires_at: started_at + OPERATION_TIMEOUT,
        }
    }

    fn request_timeout(&self) -> Result<Duration> {
        self.request_timeout_at(Instant::now())
    }

    fn ensure_remaining(&self) -> Result<()> {
        self.remaining_at(Instant::now()).map(|_| ())
    }

    fn request_timeout_at(&self, now: Instant) -> Result<Duration> {
        self.remaining_at(now)
            .map(|remaining| remaining.min(REQUEST_TIMEOUT))
    }

    fn remaining_at(&self, now: Instant) -> Result<Duration> {
        self.expires_at
            .checked_duration_since(now)
            .filter(|remaining| !remaining.is_zero())
            .ok_or(AdapterError::Transport)
    }
}

#[derive(Debug, Deserialize)]
struct ODataPage<T> {
    value: Vec<T>,
    #[serde(default, rename = "@odata.nextLink")]
    next_link: Option<String>,
}

struct Collected<T> {
    records: Vec<T>,
    has_more: bool,
}

enum PageCursor {
    SkipToken(String),
    Skip(usize),
}

impl PageCursor {
    fn identity(&self) -> String {
        match self {
            Self::SkipToken(value) => format!("token:{value}"),
            Self::Skip(value) => format!("skip:{value}"),
        }
    }
}

fn same_origin(left: &Url, right: &Url) -> bool {
    left.scheme() == right.scheme()
        && left.host_str() == right.host_str()
        && left.port_or_known_default() == right.port_or_known_default()
}

fn read_bounded_body(mut response: Response) -> Result<Vec<u8>> {
    if response
        .headers()
        .get(CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<usize>().ok())
        .is_some_and(|length| length > MAX_RESPONSE_BYTES)
    {
        return Err(AdapterError::ResponseTooLarge {
            max_bytes: MAX_RESPONSE_BYTES,
        });
    }
    let mut body = Vec::new();
    response
        .by_ref()
        .take((MAX_RESPONSE_BYTES + 1) as u64)
        .read_to_end(&mut body)
        .map_err(|_| AdapterError::Transport)?;
    if body.len() > MAX_RESPONSE_BYTES {
        return Err(AdapterError::ResponseTooLarge {
            max_bytes: MAX_RESPONSE_BYTES,
        });
    }
    Ok(body)
}

fn extract_safe_sap_code(body: &[u8]) -> Option<String> {
    let value: serde_json::Value = serde_json::from_slice(body).ok()?;
    let candidate = value
        .pointer("/error/code")
        .and_then(|value| value.as_str())
        .or_else(|| {
            value
                .pointer("/error/code/value")
                .and_then(|value| value.as_str())
        })?;
    if candidate.is_empty()
        || candidate.len() > 128
        || !candidate
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b'/'))
    {
        return None;
    }
    Some(candidate.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead as _, BufReader, Write as _};
    use std::net::{TcpListener, TcpStream};
    use std::thread;

    struct MockServer {
        origin: String,
        handle: thread::JoinHandle<Vec<String>>,
    }

    impl MockServer {
        fn start(responses: Vec<String>) -> Self {
            let responses = responses
                .into_iter()
                .map(|body| {
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    )
                    .into_bytes()
                })
                .collect();
            Self::start_raw(responses)
        }

        fn start_raw(responses: Vec<Vec<u8>>) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").expect("mock listener");
            let origin = format!("http://{}", listener.local_addr().expect("mock address"));
            let handle = thread::spawn(move || {
                let mut requests = Vec::new();
                for response in responses {
                    let (stream, _) = listener.accept().expect("mock accept");
                    requests.push(serve_once(stream, &response));
                }
                requests
            });
            Self { origin, handle }
        }

        fn finish(self) -> Vec<String> {
            self.handle.join().expect("mock server thread")
        }
    }

    fn serve_once(mut stream: TcpStream, response: &[u8]) -> String {
        let mut reader = BufReader::new(stream.try_clone().expect("clone stream"));
        let mut request_line = String::new();
        reader.read_line(&mut request_line).expect("request line");
        let mut request = request_line.trim_end().to_string();
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).expect("request header");
            if line == "\r\n" || line.is_empty() {
                break;
            }
            request.push('\n');
            request.push_str(line.trim_end());
        }
        stream.write_all(response).expect("mock response");
        request
    }

    fn raw_response(status: &str, body: &str) -> Vec<u8> {
        format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .into_bytes()
    }

    fn response(body: serde_json::Value) -> String {
        serde_json::to_string(&body).expect("response json")
    }

    fn request_url(request_line: &str) -> Url {
        let target = request_line
            .split_whitespace()
            .nth(1)
            .expect("request target");
        Url::parse(&format!("http://localhost{target}")).expect("request URL")
    }

    fn request_header<'a>(request: &'a str, name: &str) -> Option<&'a str> {
        request.lines().skip(1).find_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.eq_ignore_ascii_case(name).then(|| value.trim())
        })
    }

    #[test]
    fn operation_deadline_caps_every_page_by_the_remaining_budget() {
        let started_at = Instant::now();
        let deadline = OperationDeadline::from_start(started_at);

        assert!(OPERATION_TIMEOUT < Duration::from_secs(60));
        assert_eq!(deadline.request_timeout_at(started_at), Ok(REQUEST_TIMEOUT));
        assert_eq!(
            deadline.request_timeout_at(started_at + Duration::from_secs(30)),
            Ok(Duration::from_secs(20))
        );
        assert_eq!(
            deadline.request_timeout_at(started_at + Duration::from_secs(49)),
            Ok(Duration::from_secs(1))
        );
    }

    #[test]
    fn operation_deadline_fails_closed_at_or_after_expiry_without_sleeping() {
        let started_at = Instant::now();
        let deadline = OperationDeadline::from_start(started_at);

        assert_eq!(
            deadline.request_timeout_at(started_at + OPERATION_TIMEOUT),
            Err(AdapterError::Transport)
        );
        assert_eq!(
            deadline.request_timeout_at(started_at + OPERATION_TIMEOUT + Duration::from_millis(1)),
            Err(AdapterError::Transport)
        );
    }

    #[test]
    fn list_follows_only_the_opaque_cursor_and_rebuilds_fixed_query() {
        let next = format!(
            "{}PurchaseOrder?$skiptoken=opaque-next",
            crate::PURCHASE_ORDER_SERVICE_ROOT
        );
        let server = MockServer::start(vec![
            response(serde_json::json!({
                "value": [{"PurchaseOrder": "4500000002", "Supplier": "2000"}],
                "@odata.nextLink": next
            })),
            response(serde_json::json!({
                "value": [{"PurchaseOrder": "4500000001", "Supplier": "1000"}]
            })),
        ]);
        let client = SapClient::for_loopback_test(&server.origin).expect("test client");
        let output = client.list_purchase_orders(2).expect("list output");
        assert_eq!(output.count, 2);
        assert!(!output.has_more);

        let requests = server.finish();
        assert_eq!(requests.len(), 2);
        let first = request_url(&requests[0]);
        let second = request_url(&requests[1]);
        let first_query: std::collections::BTreeMap<_, _> = first.query_pairs().collect();
        let second_query: std::collections::BTreeMap<_, _> = second.query_pairs().collect();
        assert_eq!(
            first.path(),
            format!("{}PurchaseOrder", crate::PURCHASE_ORDER_SERVICE_ROOT)
        );
        assert_eq!(
            first_query.get("$select").map(|v| v.as_ref()),
            Some(PURCHASE_ORDER_SELECT)
        );
        assert_eq!(first_query.get("$top").map(|v| v.as_ref()), Some("3"));
        assert_eq!(
            second_query.get("$skiptoken").map(|v| v.as_ref()),
            Some("opaque-next")
        );
        assert_eq!(second_query.get("$top").map(|v| v.as_ref()), Some("2"));
        assert_eq!(
            second_query.get("$select").map(|v| v.as_ref()),
            Some(PURCHASE_ORDER_SELECT)
        );
    }

    #[test]
    fn get_uses_a_fixed_filter_and_normalizes_output() {
        let server = MockServer::start(vec![response(serde_json::json!({
            "value": [{
                "PurchaseOrder": "4500001234",
                "CompanyCode": "0001",
                "Supplier": "1000",
                "DocumentCurrency": "EUR"
            }]
        }))]);
        let client = SapClient::for_loopback_test(&server.origin).expect("test client");
        let output = client
            .get_purchase_order("4500001234")
            .expect("purchase order");
        assert_eq!(output.purchase_order.supplier.as_deref(), Some("1000"));

        let request = server.finish().pop().expect("request");
        let url = request_url(&request);
        let query: std::collections::BTreeMap<_, _> = url.query_pairs().collect();
        assert_eq!(
            query.get("$filter").map(|value| value.as_ref()),
            Some("PurchaseOrder eq '4500001234'")
        );
        assert_eq!(query.get("$top").map(|value| value.as_ref()), Some("2"));
    }

    #[test]
    fn list_uses_one_record_lookahead_to_report_more_results() {
        let server = MockServer::start(vec![response(serde_json::json!({
            "value": [
                {"PurchaseOrder": "4500000003"},
                {"PurchaseOrder": "4500000002"},
                {"PurchaseOrder": "4500000001"}
            ]
        }))]);
        let client = SapClient::for_loopback_test(&server.origin).expect("test client");
        let output = client.list_purchase_orders(2).expect("list output");
        assert_eq!(output.count, 2);
        assert_eq!(output.purchase_orders.len(), 2);
        assert!(output.has_more);

        let request = server.finish().pop().expect("request");
        let url = request_url(&request);
        let query: std::collections::BTreeMap<_, _> = url.query_pairs().collect();
        assert_eq!(query.get("$top").map(|value| value.as_ref()), Some("3"));
    }

    #[test]
    fn item_decimals_are_requested_and_preserved_as_ieee754_strings() {
        let precise = "12345678901234567890.123456789";
        let server = MockServer::start(vec![response(serde_json::json!({
            "value": [{
                "PurchaseOrder": "4500001234",
                "PurchaseOrderItem": "00010",
                "OrderQuantity": precise,
                "NetPriceAmount": precise,
                "NetPriceQuantity": precise
            }]
        }))]);
        let client = SapClient::for_loopback_test(&server.origin).expect("test client");
        let output = client
            .list_purchase_order_items("4500001234", 1)
            .expect("item output");
        let item = output.items.first().expect("one item");
        assert_eq!(item.order_quantity.as_deref(), Some(precise));
        assert_eq!(item.net_price_amount.as_deref(), Some(precise));
        assert_eq!(item.net_price_quantity.as_deref(), Some(precise));

        let request = server.finish().pop().expect("request");
        assert_eq!(request_header(&request, "accept"), Some(ACCEPT_JSON));
    }

    #[test]
    fn public_methods_reject_unbounded_inputs_before_transport() {
        let client = SapClient::for_loopback_test("http://127.0.0.1:9").expect("test client");
        assert!(matches!(
            client.get_purchase_order("4500' or true"),
            Err(AdapterError::InvalidInput { field: "id", .. })
        ));
        assert!(matches!(
            client.list_purchase_orders(0),
            Err(AdapterError::InvalidInput { field: "limit", .. })
        ));
        assert!(matches!(
            client.list_purchase_orders(usize::MAX),
            Err(AdapterError::InvalidInput { field: "limit", .. })
        ));
        assert!(matches!(
            client.list_purchase_order_items("4500' or true", 1),
            Err(AdapterError::InvalidInput { field: "id", .. })
        ));
        assert!(matches!(
            client.list_purchase_order_items("4500001234", 101),
            Err(AdapterError::InvalidInput { field: "limit", .. })
        ));
    }

    #[test]
    fn cross_origin_next_links_are_rejected() {
        let server = MockServer::start(vec![response(serde_json::json!({
            "value": [{"PurchaseOrder": "4500000002"}],
            "@odata.nextLink": "https://attacker.example/PurchaseOrder?$skiptoken=x"
        }))]);
        let client = SapClient::for_loopback_test(&server.origin).expect("test client");
        assert!(matches!(
            client.list_purchase_orders(2),
            Err(AdapterError::InvalidResponse { .. })
        ));
        let _ = server.finish();
    }

    #[test]
    fn sap_error_body_is_reduced_to_a_safe_code() {
        let body = serde_json::to_vec(&serde_json::json!({
            "error": {
                "code": "MM_PUR/001",
                "message": {"value": "sensitive business content"}
            }
        }))
        .expect("error body");
        assert_eq!(extract_safe_sap_code(&body).as_deref(), Some("MM_PUR/001"));
        assert_eq!(
            extract_safe_sap_code(br#"{"error":{"code":"unsafe code with spaces"}}"#),
            None
        );
    }

    #[test]
    fn non_success_response_keeps_only_status_and_safe_code() {
        let body = response(serde_json::json!({
            "error": {
                "code": "MM_PUR/001",
                "message": {"value": "sensitive business content"}
            }
        }));
        let server = MockServer::start_raw(vec![raw_response("503 Service Unavailable", &body)]);
        let client = SapClient::for_loopback_test(&server.origin).expect("test client");
        let error = client
            .list_purchase_orders(1)
            .expect_err("non-success response");
        assert_eq!(
            error,
            AdapterError::HttpStatus {
                status: 503,
                sap_code: Some("MM_PUR/001".to_string()),
            }
        );
        assert!(error.retryable());
        assert!(!error.to_string().contains("sensitive business content"));
        let _ = server.finish();
    }

    #[test]
    fn content_length_over_safety_cap_is_rejected_before_parsing() {
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            MAX_RESPONSE_BYTES + 1
        )
        .into_bytes();
        let server = MockServer::start_raw(vec![response]);
        let client = SapClient::for_loopback_test(&server.origin).expect("test client");
        assert_eq!(
            client.list_purchase_orders(1),
            Err(AdapterError::ResponseTooLarge {
                max_bytes: MAX_RESPONSE_BYTES,
            })
        );
        let _ = server.finish();
    }
}
