// Patches the pipeline's style JSON (`style-light.json` / `style-dark.json`) so its vector
// source points at the sideloaded Map Pack. Ported from prototype/planner-ui's
// `patchedStyleURL` (PlanStore.swift), a documented prototype bug fix: the style's
// `pmtiles://PMTILES_URL_PLACEHOLDER` placeholder needs a full URL, not a bare path -- a bare
// path here dies in CFNetwork as "unsupported URL" because `pmtiles://` wraps a URL scheme.
import Foundation

enum MapStyle {
    static func patchedStyleURL(pmtilesURL: URL, styleURL: URL) -> URL? {
        guard var text = try? String(contentsOf: styleURL, encoding: .utf8) else {
            return nil
        }
        text = text.replacingOccurrences(
            of: "pmtiles://PMTILES_URL_PLACEHOLDER",
            with: "pmtiles://file://" + pmtilesURL.path
        )
        let dst = URL(fileURLWithPath: NSTemporaryDirectory()).appendingPathComponent("wayfinder-style.json")
        guard (try? text.write(to: dst, atomically: true, encoding: .utf8)) != nil else {
            return nil
        }
        return dst
    }
}
