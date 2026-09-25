Consider this page: https://global-power-plants.datasettes.com/global-power-plants/global-power-plants?_search=plant&_facet=owner&_facet=country_long&_facet=primary_fuel

Datasette needs to run the main query for the rows on that page, a count query for the total query, then a separate query for each of those three specified facets.

This is a `_search=` query, so it needs to execute the FTS code once for the rows, again for the count, and then three more times for each of the facets.

Could running that query as a CTE and doing the other queries as part of the same large query produce significant speed improvements?