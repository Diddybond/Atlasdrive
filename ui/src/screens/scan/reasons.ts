/// Turn a decoder's error into something a photographer can act on.
///
/// "decode failed: Memory limit exceeded" is true and useless. The owner's
/// question was simply "what do the failed mean".
export function plainReason(message: string): string {
  const m = message.toLowerCase();
  if (m.includes("memory limit")) {
    return "Too large for AtlasDrive to open — usually a big 16-bit TIFF. Fixed in this version; try them again.";
  }
  if (m.includes("no such file") || m.includes("not found")) {
    return "The file was no longer there when AtlasDrive reached it — moved, renamed or deleted mid-scan.";
  }
  if (m.includes("permission")) {
    return "macOS would not let AtlasDrive read the file.";
  }
  if (m.includes("input/output") || m.includes("i/o")) {
    return "The drive could not return the data. This can mean the file or the disk is damaged.";
  }
  if (m.includes("decode failed") || m.includes("unsupported")) {
    return "Not readable as an image — either damaged, or a format AtlasDrive does not handle.";
  }
  return message;
}

