package webui

import (
	"bytes"
	"html/template"
)

// SearchPageHTML retains the historical call site while rendering the
// centralized Focus search template.
func SearchPageHTML() template.HTML {
	var content bytes.Buffer
	if err := templates.ExecuteTemplate(&content, "search.html", nil); err != nil {
		return ""
	}
	return template.HTML(content.String())
}
