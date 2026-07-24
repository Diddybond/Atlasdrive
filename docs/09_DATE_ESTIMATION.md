# 09. Date Estimation

## Principle

Never invent an exact capture date when the evidence supports only a range.

## Date candidates

Store distinct date sources:

- EXIF original capture date
- EXIF digitised date
- file creation date
- file modification date
- folder or filename date clues
- scan date clues
- user-confirmed date or range
- model-estimated range

## Evidence signals

Possible local evidence includes:

- reliable EXIF
- film or print borders
- black-and-white or colour process cues
- fading, damage and paper texture
- date stamps visible in the image
- OCR from handwritten or printed captions
- clothing and décor cues
- vehicles and products
- likely scan or photograph-of-a-photograph status

Scene cues are weak evidence and must be treated accordingly.

## Output contract

Each automatic estimate contains:

- earliest plausible date
- latest plausible date
- confidence from 0 to 1
- evidence list
- estimator version
- uncertainty explanation

Example:

```json
{
  "earliestDate": "1982-01-01",
  "latestDate": "1988-12-31",
  "confidence": 0.46,
  "evidence": [
    "likely scanned colour print",
    "no reliable EXIF capture date",
    "visible date-stamp OCR uncertain"
  ]
}
```

## Authority order

1. User-confirmed date or range
2. Trusted camera EXIF capture date
3. Trusted visible date with user confirmation
4. Strong filename or folder evidence
5. Automatic estimated range
6. File-system date shown only as a technical fact

## UI language

Use phrases such as:

- Taken on 14 June 2008
- Likely taken between 1982 and 1988
- Scanned in 2022, original date unknown
- Date uncertain

Never show an estimated midpoint as if it were a known date.
