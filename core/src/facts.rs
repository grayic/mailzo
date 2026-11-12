//! Facts an email states in a form made for machines: the schema.org JSON-LD that airlines,
//! hotels, shops and billers put in their emails. Read here, never guessed, so a question
//! like "when is my flight to Rome?" can be answered with no AI at all.

use serde_json::Value;

/// One fact an email gives.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Fact {
    /// flight, lodging, order, invoice or parcel.
    pub kind: &'static str,
    /// When it happens or is due, in Unix seconds.
    pub starts_at: Option<i64>,
    /// Where or who: the airport arrived at, the hotel, the shop.
    pub place: String,
    pub value: Value,
}

/// The JSON-LD blocks in an email's HTML.
fn blocks(html: &str) -> Vec<Value> {
    let lower = html.to_ascii_lowercase();
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(at) = lower[from..].find("application/ld+json") {
        let tag_end = match lower[from + at..].find('>') {
            Some(e) => from + at + e + 1,
            None => break,
        };
        let Some(close) = lower[tag_end..].find("</script") else {
            break;
        };
        if let Ok(v) = serde_json::from_str::<Value>(&html[tag_end..tag_end + close]) {
            out.push(v);
        }
        from = tag_end + close;
    }
    out
}

/// Every object in a block: a list, an @graph, or just one.
fn things(v: &Value, out: &mut Vec<Value>) {
    match v {
        Value::Array(list) => list.iter().for_each(|x| things(x, out)),
        Value::Object(o) => {
            if let Some(graph) = o.get("@graph") {
                things(graph, out);
            }
            if o.contains_key("@type") {
                out.push(v.clone());
            }
        }
        _ => {}
    }
}

fn text(v: &Value) -> String {
    match v {
        Value::String(s) => s.trim().to_string(),
        Value::Number(n) => n.to_string(),
        Value::Object(_) => text(&v["name"]),
        _ => String::new(),
    }
}

/// A time as written in the markup: a full date and time, or just a date (midnight UTC).
fn when(v: &Value) -> Option<i64> {
    let s = v.as_str()?.trim();
    chrono::DateTime::parse_from_rfc3339(s)
        .map(|d| d.timestamp())
        .ok()
        .or_else(|| {
            chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S")
                .ok()
                .map(|d| d.and_utc().timestamp())
        })
        .or_else(|| {
            chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d")
                .ok()
                .and_then(|d| d.and_hms_opt(0, 0, 0))
                .map(|d| d.and_utc().timestamp())
        })
}

/// "Rome Fiumicino (FCO)": a place's name, with its code when there is one.
fn named(v: &Value, code: &str) -> String {
    let name = text(v);
    let code = text(&v[code]);
    match (name.is_empty(), code.is_empty()) {
        (false, false) => format!("{name} ({code})"),
        (true, false) => code,
        _ => name,
    }
}

/// A time or date as the markup writes it: a flight leaves at the airport's own time, and a
/// date alone has no time to move to another time zone.
fn as_written(v: &Value) -> Value {
    v.as_str()
        .map(|s| Value::String(s.trim().to_string()))
        .unwrap_or(Value::Null)
}

fn amount(v: &Value) -> String {
    let price = text(&v["price"]);
    let currency = text(&v["priceCurrency"]);
    [price, currency]
        .iter()
        .filter(|p| !p.is_empty())
        .cloned()
        .collect::<Vec<_>>()
        .join(" ")
}

/// The facts an email's markup states. Nothing is guessed: no markup, no facts.
pub(crate) fn facts_in(html: &str) -> Vec<Fact> {
    let mut all = Vec::new();
    for b in blocks(html) {
        things(&b, &mut all);
    }
    all.iter()
        .filter_map(|t| {
            let kind = t["@type"].as_str().unwrap_or("");
            let r = &t["reservationFor"];
            match kind {
                "FlightReservation" => {
                    let airline = text(&r["airline"]["iataCode"]);
                    let number = text(&r["flightNumber"]);
                    let flight = format!("{airline} {number}").trim().to_string();
                    Some(Fact {
                        kind: "flight",
                        starts_at: when(&r["departureTime"]),
                        place: named(&r["arrivalAirport"], "iataCode"),
                        value: serde_json::json!({
                            "flight": flight,
                            "from": named(&r["departureAirport"], "iataCode"),
                            "to": named(&r["arrivalAirport"], "iataCode"),
                            "departs": as_written(&r["departureTime"]),
                            "arrives": as_written(&r["arrivalTime"]),
                            "booking": text(&t["reservationNumber"]),
                        }),
                    })
                }
                "LodgingReservation" => Some(Fact {
                    kind: "lodging",
                    starts_at: when(&t["checkinTime"]).or_else(|| when(&t["checkinDate"])),
                    place: [text(r), text(&r["address"]["addressLocality"])]
                        .iter()
                        .filter(|p| !p.is_empty())
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(", "),
                    value: serde_json::json!({
                        "name": text(r),
                        "checkin": if t["checkinTime"].is_string() { as_written(&t["checkinTime"]) } else { as_written(&t["checkinDate"]) },
                        "checkout": if t["checkoutTime"].is_string() { as_written(&t["checkoutTime"]) } else { as_written(&t["checkoutDate"]) },
                        "booking": text(&t["reservationNumber"]),
                    }),
                }),
                "Order" => Some(Fact {
                    kind: "order",
                    starts_at: when(&t["orderDate"]),
                    place: text(&t["merchant"]).max(text(&t["seller"])),
                    value: serde_json::json!({
                        "seller": text(&t["merchant"]).max(text(&t["seller"])),
                        "number": text(&t["orderNumber"]),
                        "amount": amount(&t["acceptedOffer"]).max(amount(&t["price"])),
                    }),
                }),
                "Invoice" => Some(Fact {
                    kind: "invoice",
                    starts_at: when(&t["paymentDueDate"]),
                    place: text(&t["provider"]).max(text(&t["broker"])),
                    value: serde_json::json!({
                        "provider": text(&t["provider"]).max(text(&t["broker"])),
                        "amount": amount(&t["totalPaymentDue"]),
                        "due": as_written(&t["paymentDueDate"]),
                    }),
                }),
                "ParcelDelivery" => Some(Fact {
                    kind: "parcel",
                    starts_at: when(&t["expectedArrivalUntil"])
                        .or_else(|| when(&t["expectedArrivalFrom"])),
                    place: text(&t["carrier"]),
                    value: serde_json::json!({
                        "carrier": text(&t["carrier"]),
                        "tracking": text(&t["trackingNumber"]),
                        "expected": as_written(&t["expectedArrivalUntil"]),
                    }),
                }),
                _ => None,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const FLIGHT: &str = r#"<html><head><script type="application/ld+json">
    {"@context":"http://schema.org","@type":"FlightReservation","reservationNumber":"RXJ34P",
     "reservationFor":{"@type":"Flight","flightNumber":"611","airline":{"@type":"Airline","name":"ITA Airways","iataCode":"AZ"},
       "departureAirport":{"@type":"Airport","name":"Dhaka","iataCode":"DAC"},"departureTime":"2030-03-04T09:40:00+06:00",
       "arrivalAirport":{"@type":"Airport","name":"Rome Fiumicino","iataCode":"FCO"},"arrivalTime":"2030-03-04T18:30:00+01:00"}}
    </script></head><body>Your trip</body></html>"#;

    #[test]
    fn a_flight_in_json_ld_becomes_a_fact() {
        let facts = facts_in(FLIGHT);
        assert_eq!(facts.len(), 1);
        let f = &facts[0];
        assert_eq!(f.kind, "flight");
        assert!(
            f.place.contains("Rome") && f.place.contains("FCO"),
            "{}",
            f.place
        );
        // 09:40 in Dhaka is 03:40 UTC.
        assert_eq!(f.starts_at, Some(1_898_826_000));
        assert_eq!(f.value["booking"], "RXJ34P");
        assert_eq!(f.value["flight"], "AZ 611");
        assert_eq!(f.value["from"], "Dhaka (DAC)");
    }

    #[test]
    fn hotels_orders_invoices_and_parcels_too() {
        let html = r#"<script type="application/ld+json">[
          {"@type":"LodgingReservation","reservationNumber":"H1","checkinTime":"2030-03-04T15:00:00+01:00",
           "reservationFor":{"@type":"LodgingBusiness","name":"Hotel Roma","address":{"addressLocality":"Rome"}}},
          {"@type":"Order","orderNumber":"112-55","orderDate":"2026-11-01T10:00:00Z","merchant":{"name":"Amazon"},
           "acceptedOffer":{"price":"29.99","priceCurrency":"USD"}},
          {"@type":"Invoice","paymentDueDate":"2026-11-20","provider":{"name":"Wyoming Registered Agent"},
           "totalPaymentDue":{"price":"125.00","priceCurrency":"USD"}},
          {"@type":"ParcelDelivery","trackingNumber":"1Z999","carrier":{"name":"UPS"},"expectedArrivalUntil":"2026-11-09"}
        ]</script>"#;
        let kinds: Vec<&str> = facts_in(html).iter().map(|f| f.kind).collect();
        assert_eq!(kinds, ["lodging", "order", "invoice", "parcel"]);
        let invoice = facts_in(html)
            .into_iter()
            .find(|f| f.kind == "invoice")
            .unwrap();
        assert_eq!(invoice.value["amount"], "125.00 USD");
        assert!(invoice.place.contains("Wyoming"));
    }

    #[test]
    fn times_and_dates_stay_as_written() {
        assert_eq!(
            facts_in(FLIGHT)[0].value["departs"],
            "2030-03-04T09:40:00+06:00"
        );
        let local = FLIGHT.replace("2030-03-04T09:40:00+06:00", "2030-03-04T09:40:00");
        assert_eq!(facts_in(&local)[0].value["departs"], "2030-03-04T09:40:00");
        let bill = r#"<script type="application/ld+json">{"@type":"Invoice","paymentDueDate":"2026-11-20",
          "provider":{"name":"X"},"totalPaymentDue":{"price":"1","priceCurrency":"USD"}}</script>"#;
        assert_eq!(facts_in(bill)[0].value["due"], "2026-11-20");
    }

    #[test]
    fn mail_without_markup_or_with_broken_markup_has_no_facts() {
        assert!(facts_in("<p>Hello</p>").is_empty());
        assert!(facts_in(r#"<script type="application/ld+json">{ not json</script>"#).is_empty());
        assert!(facts_in(
            r#"<script type="application/ld+json">{"@type":"Person","name":"Ana"}</script>"#
        )
        .is_empty());
    }
}
