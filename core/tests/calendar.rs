//! The calendar, against stand-ins for a CalDAV server and for Google and Microsoft.

use axum::extract::{Path, State};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::routing::{any, delete, get, patch, post};
use axum::{Json, Router};
use fakeimap::FakeServer;
use fakesmtp::FakeSmtp;
use mailcore::{Engine, MemoryStore, SecretStore, Store};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

const USER: &str = "me@test.test";
const PASS: &str = "secret-test-pass";
const START: i64 = 1_800_000_000;

#[derive(Default)]
struct Dav {
    events: Mutex<BTreeMap<String, String>>,
    bad_login: Mutex<bool>,
    conflict: Mutex<bool>,
    seen_auth: Mutex<Vec<String>>,
    /// Sends every request to this address instead of answering it.
    redirect_to: Mutex<Option<String>>,
    /// Answers the list of events with this instead of the list.
    bad_report: Mutex<Option<(u16, String)>>,
}

fn multistatus(inner: String) -> (StatusCode, String) {
    (StatusCode::from_u16(207).unwrap(), format!("<d:multistatus xmlns:d=\"DAV:\" xmlns:c=\"urn:ietf:params:xml:ns:caldav\">{inner}</d:multistatus>"))
}

async fn dav_or_redirect(
    state: State<Arc<Dav>>,
    method: Method,
    headers: HeaderMap,
    path: Path<String>,
    body: String,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    let elsewhere = state.redirect_to.lock().unwrap().clone();
    if let Some(to) = elsewhere {
        return (StatusCode::TEMPORARY_REDIRECT, [("location", to)], "").into_response();
    }
    dav(state, method, headers, path, body)
        .await
        .into_response()
}

async fn dav(
    State(s): State<Arc<Dav>>,
    method: Method,
    headers: HeaderMap,
    Path(path): Path<String>,
    body: String,
) -> (StatusCode, String) {
    if let Some(a) = headers.get("authorization").and_then(|v| v.to_str().ok()) {
        s.seen_auth.lock().unwrap().push(a.to_string());
    }
    if *s.bad_login.lock().unwrap() {
        return (StatusCode::UNAUTHORIZED, String::new());
    }
    match (method.as_str(), path.as_str()) {
        ("PROPFIND", "start") => multistatus("<d:response><d:href>/dav/start</d:href><d:propstat><d:prop><d:current-user-principal><d:href>/dav/me</d:href></d:current-user-principal></d:prop></d:propstat></d:response>".into()),
        ("PROPFIND", "me") => multistatus("<d:response><d:href>/dav/me</d:href><d:propstat><d:prop><c:calendar-home-set><d:href>/dav/home/</d:href></c:calendar-home-set></d:prop></d:propstat></d:response>".into()),
        ("PROPFIND", "home/") | ("PROPFIND", "home") => multistatus(
            "<d:response><d:href>/dav/home/</d:href><d:propstat><d:prop><d:resourcetype><d:collection/></d:resourcetype></d:prop></d:propstat></d:response>\
             <d:response><d:href>/dav/home/work/</d:href><d:propstat><d:prop><d:displayname>Work</d:displayname><d:resourcetype><d:collection/><c:calendar/></d:resourcetype><c:supported-calendar-component-set><c:comp name=\"VEVENT\"/></c:supported-calendar-component-set></d:prop></d:propstat></d:response>\
             <d:response><d:href>/dav/home/tasks/</d:href><d:propstat><d:prop><d:displayname>Tasks</d:displayname><d:resourcetype><d:collection/><c:calendar/></d:resourcetype><c:supported-calendar-component-set><c:comp name=\"VTODO\"/></c:supported-calendar-component-set></d:prop></d:propstat></d:response>".into(),
        ),
        ("REPORT", _) => {
            if let Some((code, text)) = s.bad_report.lock().unwrap().clone() {
                return (StatusCode::from_u16(code).unwrap(), text);
            }
            let list = s.events.lock().unwrap();
            let inner: String = list
                .iter()
                .map(|(href, data)| format!("<d:response><d:href>/dav/{href}</d:href><d:propstat><d:prop><d:getetag>\"e1\"</d:getetag><c:calendar-data><![CDATA[{data}]]></c:calendar-data></d:prop></d:propstat></d:response>"))
                .collect();
            multistatus(inner)
        }
        ("PUT", p) => {
            if *s.conflict.lock().unwrap() && headers.contains_key("if-match") {
                return (StatusCode::PRECONDITION_FAILED, String::new());
            }
            s.events.lock().unwrap().insert(p.to_string(), body);
            (StatusCode::CREATED, String::new())
        }
        ("DELETE", p) => {
            s.events.lock().unwrap().remove(p);
            (StatusCode::NO_CONTENT, String::new())
        }
        _ => (StatusCode::NOT_FOUND, String::new()),
    }
}

async fn start_dav() -> (String, Arc<Dav>) {
    let state = Arc::new(Dav::default());
    let router = Router::new()
        .route("/dav/{*path}", any(dav_or_redirect))
        .with_state(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    (url, state)
}

struct Rig {
    engine: Arc<Engine>,
    server: FakeServer,
    outgoing: FakeSmtp,
    account: i64,
    dav: Arc<Dav>,
    ai: fakeai::FakeAi,
}

async fn rig() -> Rig {
    let server = FakeServer::start(USER, PASS).await;
    server.add_folder("INBOX", &[], 1);
    server.add_folder("Sent", &["\\Sent"], 1);
    let outgoing = FakeSmtp::start(USER, PASS).await;
    let (url, dav_state) = start_dav().await;
    let ai = fakeai::FakeAi::start().await;
    let engine = Engine::new(
        Store::open_in_memory().unwrap(),
        Arc::new(MemoryStore::default()),
    )
    .with_ai_url(&ai.url)
    .with_sent_copy_wait(0)
    .with_calendar_urls(
        &format!("{url}/dav/start"),
        "http://127.0.0.1:9/g",
        "http://127.0.0.1:9/m",
    );
    let account = engine
        .call("add_account", json!({
            "email": USER, "imap_host": "127.0.0.1", "imap_port": server.addr.port(), "security": "plain",
            "password": PASS, "smtp_host": "127.0.0.1", "smtp_port": outgoing.addr.port(), "smtp_security": "plain",
        }))
        .await
        .unwrap()["id"]
        .as_i64()
        .unwrap();
    Rig {
        engine: Arc::new(engine),
        server,
        outgoing,
        account,
        dav: dav_state,
        ai,
    }
}

impl Rig {
    async fn call(&self, cmd: &str, args: Value) -> Value {
        self.engine.call(cmd, args).await.unwrap()
    }
    async fn fails(&self, cmd: &str, args: Value) -> String {
        self.engine.call(cmd, args).await.unwrap_err().to_string()
    }
    async fn events(&self) -> Vec<Value> {
        self.call(
            "calendar_events",
            json!({ "from": START - 86_400, "to": START + 30 * 86_400 }),
        )
        .await
        .as_array()
        .unwrap()
        .clone()
    }
}

fn event(calendar: &Value, title: &str) -> Value {
    json!({ "event": { "calendar_id": calendar, "title": title, "starts": START, "ends": START + 3600 } })
}

#[tokio::test]
async fn theres_always_a_calendar_of_your_own() {
    let r = rig().await;
    let list = r.call("calendar_list", json!({})).await;
    assert_eq!(list["calendars"][0]["kind"], "local");
    let id = list["calendars"][0]["id"].clone();
    let saved = r.call("calendar_save", event(&id, "Dentist")).await;
    let found = r.events().await;
    assert_eq!(found.len(), 1);
    assert_eq!(found[0]["title"], "Dentist");

    // Changing it keeps it one event.
    let mut change = event(&id, "Dentist, moved");
    change["event"]["id"] = saved["id"].clone();
    change["event"]["starts"] = json!(START + 86_400);
    change["event"]["ends"] = json!(START + 86_400 + 3600);
    r.call("calendar_save", change).await;
    let found = r.events().await;
    assert_eq!(found.len(), 1);
    assert_eq!(found[0]["title"], "Dentist, moved");

    r.call("calendar_delete", json!({ "id": saved["id"] }))
        .await;
    assert!(r.events().await.is_empty());
}

#[tokio::test]
async fn bad_events_are_refused() {
    let r = rig().await;
    let id = r.call("calendar_list", json!({})).await["calendars"][0]["id"].clone();
    assert!(r
        .fails("calendar_save", event(&id, "  "))
        .await
        .contains("name"));
    let mut backwards = event(&id, "Backwards");
    backwards["event"]["ends"] = json!(START - 1);
    assert!(r
        .fails("calendar_save", backwards)
        .await
        .contains("end after"));
    let mut bad = event(&id, "Bad repeat");
    bad["event"]["repeat"] = json!("hourly");
    assert!(r.fails("calendar_save", bad).await.contains("Bad request"));
    assert!(r
        .engine
        .call("calendar_events", json!({ "from": 10, "to": 5 }))
        .await
        .is_err());
}

#[tokio::test]
async fn repeating_events_show_each_time() {
    let r = rig().await;
    let id = r.call("calendar_list", json!({})).await["calendars"][0]["id"].clone();
    let mut weekly = event(&id, "Standup");
    weekly["event"]["repeat"] = json!("weekly");
    r.call("calendar_save", weekly).await;
    let found = r.events().await;
    assert_eq!(found.len(), 5);
    assert_eq!(
        found[1]["starts"].as_i64().unwrap() - found[0]["starts"].as_i64().unwrap(),
        7 * 86_400
    );
    assert_eq!(found[0]["repeats"], true);
}

#[tokio::test]
async fn a_connected_calendar_stays_in_step_both_ways() {
    let r = rig().await;
    r.dav.events.lock().unwrap().insert(
        "home/work/a.ics".into(),
        "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:from-server\r\nDTSTART:20270115T100000Z\r\nDTEND:20270115T110000Z\r\nSUMMARY:Server event\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n".into(),
    );
    let list = r
        .call("calendar_connect", json!({ "account_id": r.account }))
        .await;
    let cals = list["calendars"].as_array().unwrap();
    let work = cals
        .iter()
        .find(|c| c["name"] == "Work")
        .expect("the calendar was found");
    assert!(
        cals.iter().all(|c| c["name"] != "Tasks"),
        "task lists aren't calendars"
    );
    assert!(work["problem"].is_null(), "{work}");
    assert_eq!(list["can_connect"].as_array().unwrap().len(), 0);

    let found = r
        .call(
            "calendar_events",
            json!({ "from": 1_799_000_000, "to": 1_801_000_000 }),
        )
        .await;
    assert_eq!(found[0]["title"], "Server event");

    // A password goes only to the calendar server, as a normal sign-in.
    assert!(r
        .dav
        .seen_auth
        .lock()
        .unwrap()
        .iter()
        .all(|a| a.starts_with("Basic ")));

    // Made here: reaches the server.
    let saved = r
        .call("calendar_save", event(&work["id"], "Made here"))
        .await;
    assert_eq!(r.dav.events.lock().unwrap().len(), 2);
    assert!(r
        .dav
        .events
        .lock()
        .unwrap()
        .values()
        .any(|v| v.contains("Made here")));

    // Removed there: goes here on the next refresh.
    r.dav.events.lock().unwrap().remove("home/work/a.ics");
    r.call("calendar_sync", json!({})).await;
    let left = r
        .call(
            "calendar_events",
            json!({ "from": 1_799_000_000, "to": 1_801_000_000 }),
        )
        .await;
    assert!(left
        .as_array()
        .unwrap()
        .iter()
        .all(|e| e["title"] != "Server event"));

    // Removed here: goes there.
    r.call("calendar_delete", json!({ "id": saved["id"] }))
        .await;
    assert!(r.dav.events.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_change_made_elsewhere_isnt_overwritten() {
    let r = rig().await;
    r.dav.events.lock().unwrap().insert(
        "home/work/a.ics".into(),
        "BEGIN:VEVENT\r\nUID:shared\r\nDTSTART:20270115T100000Z\r\nDTEND:20270115T110000Z\r\nSUMMARY:Original\r\nEND:VEVENT\r\n".into(),
    );
    let list = r
        .call("calendar_connect", json!({ "account_id": r.account }))
        .await;
    let work = list["calendars"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "Work")
        .unwrap()
        .clone();
    let found = r
        .call(
            "calendar_events",
            json!({ "from": 1_799_000_000, "to": 1_801_000_000 }),
        )
        .await;
    *r.dav.conflict.lock().unwrap() = true;
    let mut change = event(&work["id"], "My change");
    change["event"]["id"] = found[0]["event_id"].clone();
    assert!(r
        .fails("calendar_save", change)
        .await
        .contains("changed somewhere else"));
    assert!(r
        .dav
        .events
        .lock()
        .unwrap()
        .values()
        .any(|v| v.contains("Original")));
}

#[tokio::test]
async fn a_calendar_problem_is_shown_and_doesnt_lose_what_is_here() {
    let r = rig().await;
    r.dav.events.lock().unwrap().insert(
        "home/work/a.ics".into(),
        "BEGIN:VEVENT\r\nUID:kept\r\nDTSTART:20270115T100000Z\r\nDTEND:20270115T110000Z\r\nSUMMARY:Kept\r\nEND:VEVENT\r\n".into(),
    );
    r.call("calendar_connect", json!({ "account_id": r.account }))
        .await;
    *r.dav.bad_login.lock().unwrap() = true;
    let list = r.call("calendar_sync", json!({})).await;
    let work = list["calendars"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "Work")
        .unwrap()
        .clone();
    assert!(work["problem"].is_string());
    let found = r
        .call(
            "calendar_events",
            json!({ "from": 1_799_000_000, "to": 1_801_000_000 }),
        )
        .await;
    assert_eq!(found[0]["title"], "Kept");
}

const INVITE: &str = "BEGIN:VCALENDAR\r\nMETHOD:REQUEST\r\nBEGIN:VEVENT\r\nUID:invite-1@acme\r\nDTSTART:20270601T100000Z\r\nDTEND:20270601T110000Z\r\nSUMMARY:Planning\r\nLOCATION:Room 2\r\nORGANIZER;CN=Ana:mailto:ana@acme.test\r\nATTENDEE;PARTSTAT=NEEDS-ACTION:mailto:me@test.test\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

fn invite_mail(as_attachment: bool) -> String {
    let (kind, disposition) = if as_attachment {
        (
            "application/ics; name=\"invite.ics\"",
            "Content-Disposition: attachment; filename=\"invite.ics\"\n",
        )
    } else {
        ("text/calendar; method=REQUEST; charset=utf-8", "")
    };
    format!(
        "From: Ana <ana@acme.test>\nTo: Me <{USER}>\nSubject: Invitation: Planning\nMessage-ID: <inv@acme>\n\
         MIME-Version: 1.0\nContent-Type: multipart/mixed; boundary=\"b\"\n\n--b\nContent-Type: text/plain\n\nPlease come\n\
         --b\nContent-Type: {kind}\n{disposition}\n{INVITE}\n--b--\n"
    )
}

async fn open_invite(r: &Rig) -> Value {
    r.call("sync_account", json!({ "account_id": r.account }))
        .await;
    let folders = r
        .call("list_folders", json!({ "account_id": r.account }))
        .await;
    let inbox = folders
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["role"] == "inbox")
        .unwrap()["id"]
        .clone();
    let list = r.call("list_threads", json!({ "folder_id": inbox })).await;
    let msgs = r
        .call(
            "get_thread",
            json!({ "account_id": r.account, "thread_id": list[0]["thread_id"] }),
        )
        .await;
    msgs[0]["id"].clone()
}

#[tokio::test]
async fn an_invitation_can_be_answered_from_the_email() {
    let r = rig().await;
    for as_attachment in [false, true] {
        let uid = r.server.add_message_dated(
            "INBOX",
            &invite_mail(as_attachment),
            false,
            "01-Nov-2036 10:00:00 +0000",
        );
        let id = open_invite(&r).await;
        let invite = r
            .call("invite_for_message", json!({ "message_id": id }))
            .await;
        assert_eq!(invite["title"], "Planning", "attachment: {as_attachment}");
        assert_eq!(invite["organizer"], "Ana");
        assert!(invite["my_status"].is_null() || invite["my_status"] == "ACCEPTED");

        let sent_before = r.outgoing.received().len();
        let reply = r
            .call(
                "invite_respond",
                json!({ "message_id": id, "status": "accept" }),
            )
            .await;
        assert_eq!(reply["status"], "ACCEPTED");
        let sent = r.outgoing.received();
        assert_eq!(sent.len(), sent_before + 1);
        let out = sent.last().unwrap();
        assert!(out.to.iter().any(|t| t.contains("ana@acme.test")));
        assert!(
            out.data.contains("METHOD:REPLY") && out.data.contains("PARTSTAT=ACCEPTED"),
            "{}",
            out.data
        );
        let on_it = r
            .call(
                "calendar_events",
                json!({ "from": 1_809_000_000, "to": 1_812_000_000 }),
            )
            .await;
        assert_eq!(on_it[0]["title"], "Planning");
        assert_eq!(on_it[0]["my_status"], "ACCEPTED");
        r.server.delete_message("INBOX", uid);
        r.call("sync_account", json!({ "account_id": r.account }))
            .await;
    }
}

#[tokio::test]
async fn changing_your_mind_updates_the_one_event() {
    let r = rig().await;
    r.server.add_message_dated(
        "INBOX",
        &invite_mail(true),
        false,
        "01-Nov-2036 10:00:00 +0000",
    );
    let id = open_invite(&r).await;
    r.call(
        "invite_respond",
        json!({ "message_id": id, "status": "accept" }),
    )
    .await;
    r.call(
        "invite_respond",
        json!({ "message_id": id, "status": "decline" }),
    )
    .await;
    let on_it = r
        .call(
            "calendar_events",
            json!({ "from": 1_809_000_000, "to": 1_812_000_000 }),
        )
        .await;
    assert_eq!(on_it.as_array().unwrap().len(), 1);
    assert_eq!(on_it[0]["my_status"], "DECLINED");
    r.call("invite_cancelled", json!({ "message_id": id }))
        .await;
    assert!(r
        .call(
            "calendar_events",
            json!({ "from": 1_809_000_000, "to": 1_812_000_000 })
        )
        .await
        .as_array()
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn an_ordinary_email_has_no_invitation() {
    let r = rig().await;
    r.server.add_message_dated(
        "INBOX",
        &format!("From: Ana <ana@acme.test>\nTo: {USER}\nSubject: Hi\nMessage-ID: <p@a>\nContent-Type: text/plain\n\nHello\n"),
        false,
        "01-Nov-2036 10:00:00 +0000",
    );
    let id = open_invite(&r).await;
    assert!(r
        .call("invite_for_message", json!({ "message_id": id }))
        .await
        .is_null());
    assert!(r
        .engine
        .call(
            "invite_respond",
            json!({ "message_id": id, "status": "accept" })
        )
        .await
        .is_err());
    assert!(r
        .engine
        .call(
            "invite_respond",
            json!({ "message_id": id, "status": "hmm" })
        )
        .await
        .is_err());
}

// ---- Google and Microsoft ----

#[derive(Default)]
struct Cloud {
    google: Mutex<Vec<Value>>,
    microsoft: Mutex<Vec<Value>>,
    created: Mutex<Vec<Value>>,
    deleted: Mutex<Vec<String>>,
}

async fn start_cloud() -> (String, Arc<Cloud>) {
    let state = Arc::new(Cloud::default());
    let (a, b, c, d, e, f, g, h) = (
        state.clone(),
        state.clone(),
        state.clone(),
        state.clone(),
        state.clone(),
        state.clone(),
        state.clone(),
        state.clone(),
    );
    let router = Router::new()
        .route(
            "/token",
            post(|| async { Json(json!({ "access_token": "fresh", "expires_in": 3600 })) }),
        )
        .route(
            "/g",
            get(move || {
                let a = a.clone();
                async move { Json(json!({ "items": a.google.lock().unwrap().clone() })) }
            })
            .merge(post(move |Json(body): Json<Value>| {
                let b = b.clone();
                async move {
                    b.created.lock().unwrap().push(body);
                    Json(json!({ "id": "g-new" }))
                }
            })),
        )
        .route(
            "/g/{id}",
            patch(move |Path(_id): Path<String>, Json(body): Json<Value>| {
                let c = c.clone();
                async move {
                    c.created.lock().unwrap().push(body);
                    Json(json!({ "id": "g-new" }))
                }
            })
            .merge(delete(move |Path(id): Path<String>| {
                let d = d.clone();
                async move {
                    d.deleted.lock().unwrap().push(id);
                    Json(json!({}))
                }
            })),
        )
        .route(
            "/m/calendarView",
            get(move || {
                let e = e.clone();
                async move { Json(json!({ "value": e.microsoft.lock().unwrap().clone() })) }
            }),
        )
        .route(
            "/m/events",
            post(move |Json(body): Json<Value>| {
                let f = f.clone();
                async move {
                    f.created.lock().unwrap().push(body);
                    Json(json!({ "id": "m-new" }))
                }
            }),
        )
        .route(
            "/m/events/{id}",
            delete(move |Path(id): Path<String>| {
                let g = g.clone();
                async move {
                    g.deleted.lock().unwrap().push(id);
                    Json(json!({}))
                }
            })
            .merge(patch(
                move |Path(_id): Path<String>, Json(body): Json<Value>| {
                    let h = h.clone();
                    async move {
                        h.created.lock().unwrap().push(body);
                        Json(json!({ "id": "m-new" }))
                    }
                },
            )),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    (url, state)
}

async fn signed_in(provider: &str, url: &str) -> (Engine, i64) {
    let secrets = Arc::new(MemoryStore::default());
    let store = Store::open_in_memory().unwrap();
    let mut account = store
        .insert_account(
            "me@x.test",
            "Me",
            "imap.x.test",
            993,
            mailcore::model::Security::Tls,
            "me@x.test",
        )
        .unwrap();
    account.auth = provider.to_string();
    store.save_account(&account).unwrap();
    secrets
        .set(&mailcore::secrets::account_secret(account.id), "refresh")
        .unwrap();
    let engine = Engine::new(store, secrets)
        .with_calendar_urls(
            "http://127.0.0.1:9/none",
            &format!("{url}/g"),
            &format!("{url}/m"),
        )
        .with_mail_sign_in(mailcore::signin::MailSignIn {
            provider: provider.into(),
            auth_url: format!("{url}/auth"),
            token_url: format!("{url}/token"),
            client_id: "id".into(),
            client_secret: None,
            scope: "mail".into(),
            extra_consent: None,
            extra: vec![],
            servers: None,
        });
    (engine, account.id)
}

#[tokio::test]
async fn google_calendar_reads_and_writes() {
    let (url, cloud) = start_cloud().await;
    cloud.google.lock().unwrap().push(json!({
        "id": "g1", "summary": "From Google",
        "start": { "dateTime": "2027-02-01T10:00:00Z" }, "end": { "dateTime": "2027-02-01T11:00:00Z" },
        "attendees": [{ "email": "me@x.test", "responseStatus": "accepted" }],
    }));
    cloud
        .google
        .lock()
        .unwrap()
        .push(json!({ "id": "gone", "status": "cancelled" }));
    let (engine, id) = signed_in("google", &url).await;
    let list = engine
        .call("calendar_connect", json!({ "account_id": id }))
        .await
        .unwrap();
    let cal = list["calendars"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["kind"] == "google")
        .unwrap()
        .clone();
    assert!(cal["problem"].is_null(), "{cal}");
    let found = engine
        .call(
            "calendar_events",
            json!({ "from": 1_801_000_000, "to": 1_802_000_000 }),
        )
        .await
        .unwrap();
    assert_eq!(found[0]["title"], "From Google");
    assert_eq!(found[0]["my_status"], "ACCEPTED");
    assert_eq!(found.as_array().unwrap().len(), 1);

    let made = engine.call("calendar_save", json!({ "event": { "calendar_id": cal["id"], "title": "New", "starts": START, "ends": START + 3600 } })).await.unwrap();
    assert_eq!(cloud.created.lock().unwrap()[0]["summary"], "New");
    engine
        .call("calendar_delete", json!({ "id": made["id"] }))
        .await
        .unwrap();
    assert_eq!(cloud.deleted.lock().unwrap()[0], "g-new");
}

#[tokio::test]
async fn microsoft_calendar_reads_and_writes() {
    let (url, cloud) = start_cloud().await;
    cloud.microsoft.lock().unwrap().push(json!({
        "id": "m1", "subject": "From Outlook",
        "start": { "dateTime": "2027-02-01T10:00:00.0000000" }, "end": { "dateTime": "2027-02-01T11:00:00.0000000" },
        "location": { "displayName": "Room" }, "organizer": { "emailAddress": { "address": "a@x.test" } },
    }));
    let (engine, id) = signed_in("microsoft", &url).await;
    let list = engine
        .call("calendar_connect", json!({ "account_id": id }))
        .await
        .unwrap();
    let cal = list["calendars"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["kind"] == "microsoft")
        .unwrap()
        .clone();
    assert!(cal["problem"].is_null(), "{cal}");
    let found = engine
        .call(
            "calendar_events",
            json!({ "from": 1_801_000_000, "to": 1_802_000_000 }),
        )
        .await
        .unwrap();
    assert_eq!(found[0]["title"], "From Outlook");
    assert_eq!(found[0]["location"], "Room");
    let made = engine.call("calendar_save", json!({ "event": { "calendar_id": cal["id"], "title": "New", "starts": START, "ends": START + 3600 } })).await.unwrap();
    assert_eq!(cloud.created.lock().unwrap()[0]["subject"], "New");
    engine
        .call("calendar_delete", json!({ "id": made["id"] }))
        .await
        .unwrap();
    assert_eq!(cloud.deleted.lock().unwrap()[0], "m-new");
}

// ---- What a calendar server can't talk the app into ----

const SERVER_EVENT: &str = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:kept@x\r\nDTSTART:20270101T100000Z\r\nDTEND:20270101T110000Z\r\nSUMMARY:Kept\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
const JAN_2027: (i64, i64) = (1_798_000_000, 1_800_000_000);

async fn connected_with_one_event() -> Rig {
    let r = rig().await;
    r.dav
        .events
        .lock()
        .unwrap()
        .insert("home/work/kept.ics".into(), SERVER_EVENT.into());
    r.call("calendar_connect", json!({ "account_id": r.account }))
        .await;
    r
}

async fn january(r: &Rig) -> Vec<String> {
    r.call(
        "calendar_events",
        json!({ "from": JAN_2027.0, "to": JAN_2027.1 }),
    )
    .await
    .as_array()
    .unwrap()
    .iter()
    .map(|e| e["title"].as_str().unwrap().to_string())
    .collect()
}

#[tokio::test]
async fn a_calendar_server_cant_send_the_password_somewhere_else() {
    let r = connected_with_one_event().await;
    assert_eq!(january(&r).await, ["Kept"]);
    let (other, other_state) = start_dav().await;
    *r.dav.redirect_to.lock().unwrap() = Some(format!("{other}/dav/stolen"));
    r.call("calendar_sync", json!({})).await;
    assert!(
        other_state.seen_auth.lock().unwrap().is_empty(),
        "the other server was never given the password"
    );
    assert_eq!(january(&r).await, ["Kept"], "and nothing was lost");
    let list = r.call("calendar_list", json!({})).await;
    assert!(list["calendars"]
        .as_array()
        .unwrap()
        .iter()
        .any(|c| !c["problem"].is_null()));
}

#[tokio::test]
async fn a_bad_answer_doesnt_empty_the_calendar() {
    let r = connected_with_one_event().await;
    assert_eq!(january(&r).await, ["Kept"]);
    for answer in [
        (500, "Server error".to_string()),
        (200, "<html>Please sign in</html>".to_string()),
    ] {
        *r.dav.bad_report.lock().unwrap() = Some(answer);
        r.call("calendar_sync", json!({})).await;
        assert_eq!(january(&r).await, ["Kept"]);
    }
    // An empty list that really is the list still counts.
    *r.dav.bad_report.lock().unwrap() = None;
    r.dav.events.lock().unwrap().clear();
    r.call("calendar_sync", json!({})).await;
    assert!(january(&r).await.is_empty());
}

#[tokio::test]
async fn changing_one_time_of_a_repeat_keeps_the_repeat_and_moves_them_all() {
    let r = rig().await;
    let cal = r.call("calendar_list", json!({})).await["calendars"][0]["id"].clone();
    let week = 7 * 86_400;
    let saved = r
        .call(
            "calendar_save",
            json!({ "event": {
                "calendar_id": cal, "title": "Standup",
                "starts": START, "ends": START + 3600, "repeat": "weekly",
            }}),
        )
        .await;
    let third = START + 2 * week;
    r.call(
        "calendar_save",
        json!({ "event": {
            "id": saved["id"], "calendar_id": cal, "title": "Standup",
            "starts": third + 1800, "ends": third + 1800 + 3600, "occurrence": third,
        }}),
    )
    .await;
    let list = r.events().await;
    assert!(list.len() > 2, "it still repeats");
    assert_eq!(
        list[0]["starts"],
        START + 1800,
        "the first moved by as much"
    );
    assert_eq!(list[1]["starts"], START + week + 1800);
    assert_eq!(list[0]["ends"], START + 1800 + 3600);
}

#[tokio::test]
async fn only_the_organiser_can_call_off_an_invitation() {
    let r = rig().await;
    r.server.add_message_dated(
        "INBOX",
        &invite_mail(true),
        false,
        "01-Nov-2036 10:00:00 +0000",
    );
    let id = open_invite(&r).await;
    r.call(
        "invite_respond",
        json!({ "message_id": id, "status": "accept" }),
    )
    .await;
    let cancel = INVITE.replace("METHOD:REQUEST", "METHOD:CANCEL");
    let forged = format!(
        "From: Mallory <mallory@evil.test>\nTo: Me <{USER}>\nSubject: Cancelled\nMessage-ID: <forged@evil>\n\
         MIME-Version: 1.0\nContent-Type: text/calendar; method=CANCEL; charset=utf-8\n\n{cancel}\n"
    );
    r.server
        .add_message_dated("INBOX", &forged, false, "02-Nov-2036 10:00:00 +0000");
    r.call("sync_account", json!({ "account_id": r.account }))
        .await;
    let folders = r
        .call("list_folders", json!({ "account_id": r.account }))
        .await;
    let inbox = folders
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["role"] == "inbox")
        .unwrap()["id"]
        .clone();
    let list = r.call("list_threads", json!({ "folder_id": inbox })).await;
    let thread = list
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["subject"] == "Cancelled")
        .unwrap()
        .clone();
    let msgs = r
        .call(
            "get_thread",
            json!({ "account_id": r.account, "thread_id": thread["thread_id"], "folder_id": inbox }),
        )
        .await;
    let problem = r
        .fails("invite_cancelled", json!({ "message_id": msgs[0]["id"] }))
        .await;
    assert!(problem.contains("set up"), "{problem}");
    let still = r
        .call(
            "calendar_events",
            json!({ "from": 1_809_000_000, "to": 1_812_000_000 }),
        )
        .await;
    assert_eq!(still.as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn an_invite_accepted_in_an_inbox_ai_may_not_read_never_reaches_ai() {
    let r = rig().await;
    // Kept on this Mac's own calendar, since the inbox has no calendar connected.
    let at = |h: i64| {
        (chrono::Utc::now() + chrono::Duration::hours(h))
            .format("%Y%m%dT%H%M%SZ")
            .to_string()
    };
    let mail = invite_mail(false)
        .replace("DTSTART:20270601T100000Z", &format!("DTSTART:{}", at(2)))
        .replace("DTEND:20270601T110000Z", &format!("DTEND:{}", at(3)));
    r.server
        .add_message_dated("INBOX", &mail, false, "01-Nov-2026 10:00:00 +0000");
    let id = open_invite(&r).await;
    r.call(
        "invite_respond",
        json!({ "message_id": id, "status": "accept" }),
    )
    .await;
    // The AI reads the question: a briefing before a call.
    r.call("ai_set_key", json!({ "key": fakeai::GOOD_KEY }))
        .await;
    r.ai.answer_first(
        "plan the search for: brief me before my next call",
        json!({
            "names": [], "topics": [{ "words": ["call", "meeting"] }], "after": "", "before": "",
            "answer": "summary", "waiting_on_me": false, "upcoming": true, "latest": false,
            "about_itself": false
        }),
    );
    let brief = json!({ "question": "Brief me before my next call", "quick": true });
    let found = r.call("ai_ask_sources", brief.clone()).await;
    assert!(found.to_string().contains("Planning"), "{found}");

    let mut prefs = r.call("list_accounts", json!({})).await[0]["prefs"].clone();
    prefs["ai_allowed"] = json!(false);
    r.call(
        "update_account_prefs",
        json!({ "account_id": r.account, "prefs": prefs }),
    )
    .await;
    let found = r.call("ai_ask_sources", brief).await;
    assert!(!found.to_string().contains("Planning"), "{found}");
}
