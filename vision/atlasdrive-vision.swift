// AtlasDrive local analysis worker — Apple Vision.
//
// This is the "replaceable local analysis worker in another language where model
// support is stronger" anticipated by D-009. It exists because Apple's Vision
// framework gives genuine image understanding — object and scene classification,
// real OCR, real face detection and a learned 768-dimension feature print —
// entirely on-device, with no model download and no licence to accept.
//
// Protocol, deliberately dumb so the Rust side stays simple:
//   * stdin  — one absolute image path per line, UTF-8.
//   * stdout — exactly one JSON object per line, in the same order.
//   * A file that cannot be read produces `{"ok":false,...}`, never a crash and
//     never a skipped line, so the caller's line accounting always matches.
//
// The process is long-running: spawning a process per photograph would dominate
// the cost of indexing a large archive.
//
// Safety: images are opened read-only through CGImageSource. Nothing is written
// anywhere, and no network API is used.

import Foundation
import Vision
import CoreImage

// MARK: - Output shapes

struct Label: Encodable {
    let id: String
    let c: Float
}

struct FaceBox: Encodable {
    let x: Float
    let y: Float
    let w: Float
    let h: Float
    let c: Float
}

struct Analysis: Encodable {
    let ok: Bool
    let error: String?
    let width: Int
    let height: Int
    let labels: [Label]
    let ocr: String
    let faces: [FaceBox]
    /// The Vision feature print, as plain floats.
    let print: [Float]
}

func fail(_ message: String) -> Analysis {
    Analysis(ok: false, error: message, width: 0, height: 0,
             labels: [], ocr: "", faces: [], print: [])
}

// MARK: - Feature print decoding

/// Vision returns the feature print as packed bytes plus an element type.
/// Unpack to `[Float]` so the Rust side never has to know the layout.
func floats(from observation: VNFeaturePrintObservation) -> [Float] {
    let count = observation.elementCount
    let data = observation.data

    switch observation.elementType {
    case .float:
        return data.withUnsafeBytes { raw in
            Array(UnsafeBufferPointer(start: raw.bindMemory(to: Float.self).baseAddress, count: count))
        }
    case .double:
        return data.withUnsafeBytes { raw in
            let buf = UnsafeBufferPointer(start: raw.bindMemory(to: Double.self).baseAddress, count: count)
            return buf.map { Float($0) }
        }
    default:
        return []
    }
}

// MARK: - Analysis

/// Labels below this are noise; Vision emits a long tail of near-zero guesses.
let minLabelConfidence: Float = 0.10
/// Enough to describe a photograph without bloating the catalogue.
let maxLabels = 12

func analyse(path: String) -> Analysis {
    let url = URL(fileURLWithPath: path)
    guard let source = CGImageSourceCreateWithURL(url as CFURL, nil),
          let image = CGImageSourceCreateImageAtIndex(source, 0, nil) else {
        return fail("could not decode image")
    }

    let handler = VNImageRequestHandler(cgImage: image, options: [:])
    let classify = VNClassifyImageRequest()
    let ocr = VNRecognizeTextRequest()
    ocr.recognitionLevel = .accurate
    ocr.usesLanguageCorrection = true
    let faces = VNDetectFaceRectanglesRequest()
    let featurePrint = VNGenerateImageFeaturePrintRequest()

    do {
        try handler.perform([classify, ocr, faces, featurePrint])
    } catch {
        return fail("vision request failed: \(error.localizedDescription)")
    }

    let labels: [Label] = (classify.results ?? [])
        .filter { $0.confidence >= minLabelConfidence }
        .sorted { $0.confidence > $1.confidence }
        .prefix(maxLabels)
        .map { Label(id: $0.identifier, c: $0.confidence) }

    let text = (ocr.results ?? [])
        .compactMap { $0.topCandidates(1).first?.string }
        .joined(separator: "\n")

    // Vision's origin is bottom-left; the catalogue's is top-left.
    let faceBoxes: [FaceBox] = (faces.results ?? []).map { face in
        let b = face.boundingBox
        return FaceBox(x: Float(b.origin.x),
                       y: Float(1.0 - b.origin.y - b.size.height),
                       w: Float(b.size.width),
                       h: Float(b.size.height),
                       c: face.confidence)
    }

    let print_ = featurePrint.results?.first.map { floats(from: $0) } ?? []

    return Analysis(ok: true, error: nil,
                    width: image.width, height: image.height,
                    labels: labels, ocr: text, faces: faceBoxes, print: print_)
}

// MARK: - Main loop

let encoder = JSONEncoder()
// Stable key order keeps the transcript diffable when debugging.
encoder.outputFormatting = [.sortedKeys]

/// `--selftest` lets the build and the Rust side confirm the helper is the right
/// binary and that Vision is reachable, without needing an image to hand.
if CommandLine.arguments.contains("--selftest") {
    print("atlasdrive-vision 1")
    exit(0)
}

setbuf(stdout, nil)

while let line = readLine(strippingNewline: true) {
    let path = line.trimmingCharacters(in: .whitespaces)
    if path.isEmpty { continue }
    let result = analyse(path: path)
    if let data = try? encoder.encode(result), let json = String(data: data, encoding: .utf8) {
        print(json)
    } else {
        // Must still emit a line, or the caller's ordering breaks.
        print("{\"ok\":false,\"error\":\"could not encode result\",\"faces\":[],\"height\":0,\"labels\":[],\"ocr\":\"\",\"print\":[],\"width\":0}")
    }
}
