# 07. Visual Search and Tagging

## Search modes

### Text and metadata

Search:

- filenames
- folder paths
- OCR text
- user notes
- user tags
- automatic tags
- people names and aliases
- drive names and numbers

### Natural-language visual search

Example queries:

- bike images
- children opening presents
- old black-and-white family portrait
- people standing outside a church
- red car in front of a house

The query is embedded locally using the same compatible visual-text model family as the image embeddings.

### Similar-image search

From a selected image, return visually related images across all indexed drives.

### Combined filters

Allow visual similarity to be combined with:

- drive number
- person
- date range
- connected or offline state
- scanned-print likelihood
- file type
- user-confirmed tag

## Tag hierarchy

Keep tag provenance explicit.

### User-confirmed tags

Highest authority. Never removed by model reprocessing.

### Person tags

Based on human-confirmed identity links.

### Automatic concept tags

Generated from local analysis with confidence and model version.

### System tags

Examples:

- offline original
- likely duplicate
- unreadable
- likely scan
- date uncertain
- needs face review

## Indexing strategy

Use a vector index suitable for a local single-user catalogue. The exact implementation may use a SQLite vector extension or an adjacent local vector store, but it must:

- persist locally
- support model-version partitions
- rebuild deterministically
- avoid cloud services
- survive application upgrades

## Search-result requirements

Each result card must show:

- thumbnail
- source filename
- drive number
- drive name
- online or offline status
- likely date range when useful
- matched people or concepts
- confidence or match-strength indicator

Do not present probabilistic labels as certain facts.

## Relevance feedback

Allow the user to mark a result relevant or not relevant. Store this as local feedback for later ranking improvements without altering the original file or automatic raw model output.
