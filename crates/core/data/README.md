# Bundled data

## places.tsv.gz

Every populated place in the world with at least 1,000 people: name, county or
district, region or state, and country code, with its position. It is used to
turn a photograph's GPS position into place names, offline (D-106).

Source: [GeoNames](https://www.geonames.org), `cities1000`, licensed under
[Creative Commons Attribution 4.0](https://creativecommons.org/licenses/by/4.0/).
Taken from the copy distributed with the `reverse_geocoder` project
(`rg_cities1000.csv`), which joins GeoNames' admin1 and admin2 names onto each
place. Converted to tab-separated values, with positions rounded to four
decimal places (about 11 m), and gzip-compressed.
