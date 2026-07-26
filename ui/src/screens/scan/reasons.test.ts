import { plainReason } from "./reasons";

describe("plain-language failure reasons", () => {
  /// The exact message that failed 232 photographs on the owner's Drive 2.
  it("explains the memory limit as the fixable thing it is", () => {
    const said = plainReason("decode failed: Memory limit exceeded");
    expect(said).toMatch(/TIFF/);
    expect(said).toMatch(/try them again/i);
    // The raw error is the thing being replaced; it must not survive.
    expect(said).not.toMatch(/Memory limit exceeded/);
  });

  it("distinguishes a damaged drive from an unreadable format", () => {
    expect(plainReason("Input/output error (os error 5)")).toMatch(/damaged/i);
    expect(plainReason("decode failed: unsupported format")).toMatch(/format AtlasDrive does not handle/);
  });

  it("says plainly when the file simply was not there", () => {
    expect(plainReason("No such file or directory")).toMatch(/no longer there/);
  });

  it("says plainly when macOS refused", () => {
    expect(plainReason("Permission denied (os error 13)")).toMatch(/would not let AtlasDrive read/);
  });

  /// Anything unrecognised is passed through rather than replaced with a
  /// reassuring guess — a wrong explanation is worse than a technical one.
  it("passes an unrecognised message through untouched", () => {
    expect(plainReason("something nobody anticipated")).toBe("something nobody anticipated");
  });

  it("is not confused by capitalisation", () => {
    expect(plainReason("DECODE FAILED: MEMORY LIMIT EXCEEDED")).toMatch(/TIFF/);
  });
});
