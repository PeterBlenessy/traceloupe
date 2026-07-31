/** Common `ProductType` → marketing-name mappings; falls back to the raw id.
 *  Shared by the Device view and the Health timezone timeline. */
const MODEL_NAMES: Record<string, string> = {
  "iPhone8,1": "iPhone 6s",
  "iPhone8,4": "iPhone SE (1st gen)",
  "iPhone10,3": "iPhone X",
  "iPhone10,6": "iPhone X",
  "iPhone11,2": "iPhone XS",
  "iPhone11,8": "iPhone XR",
  "iPhone12,1": "iPhone 11",
  "iPhone12,3": "iPhone 11 Pro",
  "iPhone12,5": "iPhone 11 Pro Max",
  "iPhone12,8": "iPhone SE (2nd gen)",
  "iPhone13,1": "iPhone 12 mini",
  "iPhone13,2": "iPhone 12",
  "iPhone13,3": "iPhone 12 Pro",
  "iPhone13,4": "iPhone 12 Pro Max",
  "iPhone14,2": "iPhone 13 Pro",
  "iPhone14,3": "iPhone 13 Pro Max",
  "iPhone14,4": "iPhone 13 mini",
  "iPhone14,5": "iPhone 13",
  "iPhone14,6": "iPhone SE (3rd gen)",
  "iPhone14,7": "iPhone 14",
  "iPhone14,8": "iPhone 14 Plus",
  "iPhone15,2": "iPhone 14 Pro",
  "iPhone15,3": "iPhone 14 Pro Max",
  "iPhone15,4": "iPhone 15",
  "iPhone15,5": "iPhone 15 Plus",
  "iPhone16,1": "iPhone 15 Pro",
  "iPhone16,2": "iPhone 15 Pro Max",

  // Apple Watch. Health data carries the WATCH's product type as readily as the
  // phone's, so a device history is full of these and "Watch4,3" tells a reader
  // nothing.
  //
  // Series only, no case size: `Watch4,3` is a 44mm Series 4, but the size is
  // not what identifies the device to a reader, and pretending to a precision
  // the mapping does not reliably have would be worse than omitting it.
  //
  // Deliberately stops at Series 5. Later identifiers interleave SE, Ultra and
  // numbered series within one major version (the `Watch6,*` range spans three
  // product lines), and a wrong entry here MISLABELS someone's device — a
  // failure worth more than the convenience of a full table. `modelName` falls
  // back to the raw identifier, which is honest, so the gap costs nothing but
  // polish until the ranges are verified against Apple's own list.
  "Watch1,1": "Apple Watch (1st gen)",
  "Watch1,2": "Apple Watch (1st gen)",
  "Watch2,6": "Apple Watch Series 1",
  "Watch2,7": "Apple Watch Series 1",
  "Watch2,3": "Apple Watch Series 2",
  "Watch2,4": "Apple Watch Series 2",
  "Watch3,1": "Apple Watch Series 3",
  "Watch3,2": "Apple Watch Series 3",
  "Watch3,3": "Apple Watch Series 3",
  "Watch3,4": "Apple Watch Series 3",
  "Watch4,1": "Apple Watch Series 4",
  "Watch4,2": "Apple Watch Series 4",
  "Watch4,3": "Apple Watch Series 4",
  "Watch4,4": "Apple Watch Series 4",
  "Watch5,1": "Apple Watch Series 5",
  "Watch5,2": "Apple Watch Series 5",
  "Watch5,3": "Apple Watch Series 5",
  "Watch5,4": "Apple Watch Series 5",
};

export function modelName(productType: string | null): string | null {
  if (!productType) return null;
  return MODEL_NAMES[productType] ?? productType;
}
