// Apple Intelligence for Mailzo: the Mac's own language model, through Apple's
// FoundationModels framework. Built into the app by build.rs only when the SDK has it, and
// it only answers on a Mac that has Apple Intelligence switched on.
//
// A few plain C functions, so the Rust side can call it:
//   mailzo_ai_state()                      0 ready, 1 not this Mac, 2 switched off, 3 downloading
//   mailzo_ai_ask(system, prompt, schema, &answer)
//                                          0 answered, 1 too long, 2 refused, 3 not ready, 4 failed
//   mailzo_ai_free(answer)

import Foundation
import FoundationModels

private final class Box: @unchecked Sendable {
    var code: Int32 = 4
    var text: String = ""
}

@_cdecl("mailzo_ai_state")
public func mailzoAiState() -> Int32 {
    guard #available(macOS 26.0, *) else { return 1 }
    switch SystemLanguageModel.default.availability {
    case .available:
        return 0
    case .unavailable(let reason):
        switch reason {
        case .deviceNotEligible: return 1
        case .appleIntelligenceNotEnabled: return 2
        case .modelNotReady: return 3
        @unknown default: return 1
        }
    @unknown default:
        return 1
    }
}

@_cdecl("mailzo_ai_free")
public func mailzoAiFree(_ text: UnsafeMutablePointer<CChar>?) {
    free(text)
}

@_cdecl("mailzo_ai_ask")
public func mailzoAiAsk(
    _ system: UnsafePointer<CChar>,
    _ prompt: UnsafePointer<CChar>,
    _ schema: UnsafePointer<CChar>,
    _ answer: UnsafeMutablePointer<UnsafeMutablePointer<CChar>?>
) -> Int32 {
    answer.pointee = nil
    guard #available(macOS 26.0, *) else { return 3 }
    let system = String(cString: system)
    let prompt = String(cString: prompt)
    let schema = String(cString: schema)
    let box = Box()
    let done = DispatchSemaphore(value: 0)
    Task.detached {
        (box.code, box.text) = await ask(system: system, prompt: prompt, schema: schema)
        done.signal()
    }
    done.wait()
    if box.code == 0 {
        answer.pointee = strdup(box.text)
    }
    return box.code
}

@available(macOS 26.0, *)
private func ask(system: String, prompt: String, schema: String) async -> (Int32, String) {
    let session = LanguageModelSession(instructions: system)
    let options = GenerationOptions(temperature: 0.3)
    do {
        // Held to the shape asked for, so the answer is always the JSON the app expects.
        if let shape = generationSchema(from: schema) {
            let reply = try await session.respond(to: prompt, schema: shape, options: options)
            return (0, reply.content.jsonString)
        }
        let reply = try await session.respond(
            to: prompt + "\n\nReply with only a JSON object matching this JSON schema: " + schema,
            options: options
        )
        return (0, reply.content)
    } catch let error as LanguageModelSession.GenerationError {
        switch error {
        case .exceededContextWindowSize: return (1, "")
        case .guardrailViolation, .refusal: return (2, "")
        case .assetsUnavailable: return (3, "")
        default: return (4, "")
        }
    } catch {
        // macOS 27 says why in a type of its own.
        #if compiler(>=6.4)
        if #available(macOS 27.0, *), let error = error as? LanguageModelError {
            switch error {
            case .contextSizeExceeded: return (1, "")
            case .guardrailViolation, .refusal: return (2, "")
            default: return (4, "")
            }
        }
        #endif
        return (4, "")
    }
}

// MARK: - JSON schema → Apple's generation schema

@available(macOS 26.0, *)
private func generationSchema(from text: String) -> GenerationSchema? {
    guard let data = text.data(using: .utf8),
          let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any]
    else { return nil }
    var names = 0
    guard let root = dynamic(json, named: "Answer", counter: &names) else { return nil }
    return try? GenerationSchema(root: root, dependencies: [])
}

/// JSON schema allows "anyOf: [X, null]" for something that may be missing.
private func withoutNull(_ json: [String: Any]) -> ([String: Any], Bool) {
    if let options = json["anyOf"] as? [[String: Any]] {
        let real = options.filter { ($0["type"] as? String) != "null" }
        if real.count == 1 { return (real[0], real.count != options.count) }
    }
    if let types = json["type"] as? [String], types.contains("null"),
       let other = types.first(where: { $0 != "null" }) {
        var copy = json
        copy["type"] = other
        return (copy, true)
    }
    return (json, false)
}

@available(macOS 26.0, *)
private func dynamic(_ json: [String: Any], named name: String, counter: inout Int) -> DynamicGenerationSchema? {
    let (json, _) = withoutNull(json)
    let about = json["description"] as? String
    if let choices = json["enum"] as? [String] {
        counter += 1
        return DynamicGenerationSchema(name: "\(name)\(counter)", description: about, anyOf: choices)
    }
    switch json["type"] as? String {
    case "object":
        let properties = json["properties"] as? [String: Any] ?? [:]
        let required = Set(json["required"] as? [String] ?? [])
        // Kept in the schema's own order where it says one, so the model writes them in turn.
        let order = (json["required"] as? [String] ?? []) + properties.keys.sorted().filter { !required.contains($0) }
        var fields: [DynamicGenerationSchema.Property] = []
        for key in order {
            guard let raw = properties[key] as? [String: Any] else { continue }
            let (inner, nullable) = withoutNull(raw)
            guard let schema = dynamic(inner, named: key.capitalized, counter: &counter) else { return nil }
            fields.append(DynamicGenerationSchema.Property(
                name: key,
                description: raw["description"] as? String ?? inner["description"] as? String,
                schema: schema,
                isOptional: nullable || !required.contains(key)
            ))
        }
        counter += 1
        return DynamicGenerationSchema(name: "\(name)\(counter)", description: about, properties: fields)
    case "array":
        guard let items = json["items"] as? [String: Any],
              let inner = dynamic(items, named: name + "Item", counter: &counter)
        else { return nil }
        return DynamicGenerationSchema(
            arrayOf: inner,
            minimumElements: json["minItems"] as? Int,
            maximumElements: json["maxItems"] as? Int
        )
    case "string":
        return DynamicGenerationSchema(type: String.self)
    case "integer":
        return DynamicGenerationSchema(type: Int.self)
    case "number":
        return DynamicGenerationSchema(type: Double.self)
    case "boolean":
        return DynamicGenerationSchema(type: Bool.self)
    default:
        return nil
    }
}
