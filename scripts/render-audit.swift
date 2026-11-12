// Runs the email display check in WebKit itself, the engine the Mac app uses. Every network request
// is refused. Build and run:  swiftc scripts/render-audit.swift -o /tmp/ra && /tmp/ra job.json out.jsonl
import AppKit
import WebKit

struct Row: Decodable { let id: Int; let domain: String; let html: String }
struct Job: Decodable { let prepareCode: String; let auditCode: String; let widths: [Int]; let rows: [Row] }

let args = CommandLine.arguments
let job = try! JSONDecoder().decode(Job.self, from: Data(contentsOf: URL(fileURLWithPath: args[1])))
FileManager.default.createFile(atPath: args[2], contents: nil)
let out = FileHandle(forWritingAtPath: args[2])!

let app = NSApplication.shared
app.setActivationPolicy(.prohibited)

@MainActor
func run() async {
    let rules = """
    [{"trigger":{"url-filter":".*"},"action":{"type":"block"}},
     {"trigger":{"url-filter":"^about:"},"action":{"type":"ignore-previous-rules"}},
     {"trigger":{"url-filter":"^https://render-audit\\\\.test/$"},"action":{"type":"ignore-previous-rules"}}]
    """
    let list = try! await WKContentRuleListStore.default()!.compileContentRuleList(forIdentifier: "audit", encodedContentRuleList: rules)!
    let config = WKWebViewConfiguration()
    config.websiteDataStore = .nonPersistent()
    config.userContentController.add(list)
    let view = WKWebView(frame: NSRect(x: 0, y: 0, width: 1000, height: 1000), configuration: config)
    let window = NSWindow(contentRect: NSRect(x: -4000, y: -4000, width: 1000, height: 1000), styleMask: .borderless, backing: .buffered, defer: false)
    window.contentView = view
    window.orderBack(nil)
    view.loadHTMLString("<!doctype html><title>audit</title><body></body>", baseURL: URL(string: "https://render-audit.test/"))
    while view.isLoading { try? await Task.sleep(nanoseconds: 50_000_000) }
    try? await Task.sleep(nanoseconds: 300_000_000)
    _ = try? await view.evaluateJavaScript("(function(){" + job.prepareCode + "\n})(); true")
    var done = 0
    for row in job.rows {
        var line: [String: Any] = ["id": row.id, "domain": row.domain]
        do {
            let body = "const audit = (" + job.auditCode + "); return JSON.stringify(await audit(html, widths));"
            let result = try await view.callAsyncJavaScript(body, arguments: ["html": row.html, "widths": job.widths], in: nil, contentWorld: .page)
            if let text = result as? String, let parsed = try? JSONSerialization.jsonObject(with: Data(text.utf8)) as? [String: Any] {
                parsed.forEach { line[$0.key] = $0.value }
            }
        } catch {
            line["why"] = String(describing: error).prefix(160).description
        }
        out.write(try! JSONSerialization.data(withJSONObject: line))
        out.write(Data("\n".utf8))
        done += 1
        if done % 200 == 0 { print(done, "of", job.rows.count) }
    }
    print("checked", done)
    exit(0)
}
Task { @MainActor in await run() }
app.run()
