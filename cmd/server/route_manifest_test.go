package main

import (
	"go/ast"
	"go/parser"
	"go/token"
	"runtime"
	"sort"
	"strconv"
	"strings"
	"testing"
)

func TestCoreRouteManifestRequiresClassification(t *testing.T) {
	_, testFile, _, ok := runtime.Caller(0)
	if !ok {
		t.Fatal("resolve test source")
	}
	mainFile := strings.TrimSuffix(testFile, "route_manifest_test.go") + "main.go"
	parsed, err := parser.ParseFile(token.NewFileSet(), mainFile, nil, 0)
	if err != nil {
		t.Fatal(err)
	}
	var actual []string
	ast.Inspect(parsed, func(node ast.Node) bool {
		call, ok := node.(*ast.CallExpr)
		if !ok {
			return true
		}
		selector, ok := call.Fun.(*ast.SelectorExpr)
		if !ok {
			return true
		}
		router, ok := selector.X.(*ast.SelectorExpr)
		if !ok || router.Sel.Name != "Router" {
			return true
		}
		identifier, ok := router.X.(*ast.Ident)
		if !ok || identifier.Name != "srv" {
			return true
		}
		method := strings.ToUpper(selector.Sel.Name)
		pathArgument := 0
		if selector.Sel.Name == "Method" {
			if len(call.Args) < 2 {
				return true
			}
			methodLiteral, ok := call.Args[0].(*ast.BasicLit)
			if !ok {
				return true
			}
			value, err := strconv.Unquote(methodLiteral.Value)
			if err != nil {
				return true
			}
			method = value
			pathArgument = 1
		} else if selector.Sel.Name == "Handle" {
			method = "ANY"
		}
		if pathArgument >= len(call.Args) {
			return true
		}
		pathLiteral, ok := call.Args[pathArgument].(*ast.BasicLit)
		if !ok {
			return true
		}
		path, err := strconv.Unquote(pathLiteral.Value)
		if err == nil {
			actual = append(actual, method+" "+path)
		}
		return true
	})

	expected := strings.FieldsFunc(`
		GET /
		GET /login
		GET /auth/oidc/callback
		GET /assets/app.v1.css
		GET /assets/codex-panel.js
		GET /assets/favicon.svg
		GET /assets/fontawesome/*
		GET /home
		GET /search
		GET /settings
		GET /settings/*
		GET /api/me
		POST /api/search/query
		POST /api/search/matches
		ANY /api/search/preferences
		GET /api/notifications
		DELETE /api/notifications
		DELETE /api/notifications/{id}
		ANY /api/notifications/preferences
		GET /api/notifications/stream
		GET /api/dashboard/catalog
		ANY /api/dashboard/layout
		GET /api/dashboard/widgets/{id}
		GET /api/dashboard/widgets/{id}/settings
		PUT /api/dashboard/widgets/{id}/settings
		ANY /api/settings/appearance
		POST /api/settings/appearance/preview
		GET /api/appearance/wallpaper
		GET /assets/text-editor.js
		GET /assets/dashboard.js
		GET /assets/dar-ui.js
		GET /assets/notifications.js
		GET /assets/search.js
		GET /assets/settings.js
		GET /assets/shell.js
		POST /logout
		GET /health
		POST /api/settings/personal
		GET /api/settings/time-format
		POST /api/settings/time-format
		GET /api/settings/files
		POST /api/settings/files
		GET /api/settings/calendar
		POST /api/settings/calendar
		GET /api/settings/app-passwords
		POST /api/settings/app-passwords
		DELETE /api/settings/app-passwords/{id}
		ANY /.well-known/carddav
		ANY /.well-known/caldav
		ANY /remote.php/dav
		ANY /remote.php/dav/*
		PROPFIND /
		OPTIONS /
	`, func(r rune) bool { return r == '\n' })
	for index := range expected {
		expected[index] = strings.TrimSpace(expected[index])
	}
	expected = compactStrings(expected)
	sort.Strings(actual)
	sort.Strings(expected)
	if strings.Join(actual, "\n") != strings.Join(expected, "\n") {
		t.Fatalf("core route manifest changed; classify and test every new route\nactual:\n%s\nexpected:\n%s",
			strings.Join(actual, "\n"), strings.Join(expected, "\n"))
	}
}

func compactStrings(values []string) []string {
	result := values[:0]
	for _, value := range values {
		if value != "" {
			result = append(result, value)
		}
	}
	return result
}
