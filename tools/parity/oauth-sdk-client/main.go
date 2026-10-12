// Command oauth-sdk-client completes an operator-approved authorization_code
// flow against a running Rust Host Agent with the real go-sdk client
// (secure-oauth-issuance task 5.3). The authorization code fetcher stands in
// for the browser: it opens the approval page, approves the user code through
// the agent's own `oauth approve` CLI (the trusted channel), and reads the
// redirect from /oauth/authorize/status. Everything else, from protected
// resource discovery to the code exchange and the authenticated MCP session,
// is the SDK's.
package main

import (
	"context"
	"encoding/json"
	"flag"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"os"
	"os/exec"
	"regexp"
	"time"

	"github.com/modelcontextprotocol/go-sdk/auth"
	"github.com/modelcontextprotocol/go-sdk/mcp"
	"github.com/modelcontextprotocol/go-sdk/oauthex"
)

var (
	endpoint = flag.String("endpoint", "", "MCP endpoint of the agent")
	agent    = flag.String("agent", "", "Rust agent binary, for `oauth approve`")
	stateDir = flag.String("state-dir", "", "the agent's state directory")
	redirect = flag.String("redirect", "http://127.0.0.1:49152/callback", "registered loopback redirect")
	deny     = flag.Bool("deny", false, "deny the request instead (negative control)")
)

var (
	requestRe = regexp.MustCompile(`request=([0-9a-f]+)`)
	codeRe    = regexp.MustCompile(`oauth approve ([A-Z0-9]{4}-[A-Z0-9]{4})`)
)

type receipt struct {
	ApprovalPage struct {
		Status       int    `json:"status"`
		CacheControl string `json:"cacheControl"`
	} `json:"approvalPage"`
	Approve struct {
		Exit int `json:"exit"`
	} `json:"approve"`
	Redirect struct {
		Status   int    `json:"status"`
		HasCode  bool   `json:"hasCode"`
		StateSet bool   `json:"stateSet"`
		Iss      string `json:"iss"`
		Error    string `json:"error,omitempty"`
	} `json:"redirect"`
	Session struct {
		Connected bool   `json:"connected"`
		ToolCount int    `json:"toolCount"`
		Error     string `json:"error,omitempty"`
	} `json:"session"`
}

func main() {
	flag.Parse()
	var out receipt
	noFollow := &http.Client{Timeout: 10 * time.Second, CheckRedirect: func(*http.Request, []*http.Request) error {
		return http.ErrUseLastResponse
	}}
	fetch := func(ctx context.Context, args *auth.AuthorizationArgs) (*auth.AuthorizationResult, error) {
		page, err := noFollow.Get(args.URL)
		if err != nil {
			return nil, err
		}
		body, _ := io.ReadAll(page.Body)
		page.Body.Close()
		out.ApprovalPage.Status = page.StatusCode
		out.ApprovalPage.CacheControl = page.Header.Get("Cache-Control")
		request, code := requestRe.FindSubmatch(body), codeRe.FindSubmatch(body)
		if request == nil || code == nil {
			return nil, fmt.Errorf("approval page without a request id or user code (status %d)", page.StatusCode)
		}
		decision := []string{"oauth", "approve", string(code[1]), "--once", "--state-dir", *stateDir}
		if *deny {
			decision = []string{"oauth", "deny", string(code[1]), "--state-dir", *stateDir}
		}
		approve := exec.CommandContext(ctx, *agent, decision...)
		approve.Stdout, approve.Stderr = io.Discard, os.Stderr
		if err := approve.Run(); err != nil {
			out.Approve.Exit = approve.ProcessState.ExitCode()
			return nil, fmt.Errorf("oauth approve: %w", err)
		}
		authorize, _ := url.Parse(args.URL)
		status := authorize.ResolveReference(&url.URL{Path: "/oauth/authorize/status", RawQuery: "request=" + string(request[1])})
		reply, err := noFollow.Get(status.String())
		if err != nil {
			return nil, err
		}
		reply.Body.Close()
		out.Redirect.Status = reply.StatusCode
		location, err := url.Parse(reply.Header.Get("Location"))
		if err != nil {
			return nil, err
		}
		query := location.Query()
		out.Redirect.HasCode = query.Get("code") != ""
		out.Redirect.StateSet = query.Get("state") != ""
		out.Redirect.Iss = query.Get("iss")
		out.Redirect.Error = query.Get("error")
		return &auth.AuthorizationResult{Code: query.Get("code"), State: query.Get("state"), Iss: query.Get("iss")}, nil
	}
	handler, err := auth.NewAuthorizationCodeHandler(&auth.AuthorizationCodeHandlerConfig{
		PreregisteredClient:      &oauthex.ClientCredentials{ClientID: "host-agent-bootstrap"},
		RedirectURL:              *redirect,
		AuthorizationCodeFetcher: fetch,
	})
	if err != nil {
		fail(&out, err)
	}
	ctx, cancel := context.WithTimeout(context.Background(), 60*time.Second)
	defer cancel()
	client := mcp.NewClient(&mcp.Implementation{Name: "parity-go-sdk-oauth", Version: "1.0.0"}, nil)
	session, err := client.Connect(ctx, &mcp.StreamableClientTransport{Endpoint: *endpoint, OAuthHandler: handler}, nil)
	if err != nil {
		fail(&out, err)
	}
	defer session.Close()
	out.Session.Connected = true
	tools, err := session.ListTools(ctx, nil)
	if err != nil {
		fail(&out, err)
	}
	out.Session.ToolCount = len(tools.Tools)
	emit(out)
}

func fail(out *receipt, err error) {
	out.Session.Error = err.Error()
	emit(*out)
	os.Exit(1)
}

func emit(out receipt) {
	encoder := json.NewEncoder(os.Stdout)
	encoder.SetIndent("", " ")
	_ = encoder.Encode(out)
}
