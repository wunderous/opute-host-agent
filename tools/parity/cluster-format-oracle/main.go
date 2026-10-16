package main

import (
	"encoding/base64"
	"encoding/json"
	"fmt"
	"strings"
	"time"
)

func ShellQuote(value string) string { return "'" + strings.ReplaceAll(value, "'", `'\''`) + "'" }

type InstallHelmChartArgs struct{ VMName, ReleaseName, ChartSource, Namespace, Repo, Values string }

func indentYAMLBlock(values string) string {
	lines := strings.Split(strings.TrimRight(values, "\n"), "\n")
	for i, line := range lines {
		lines[i] = "    " + line
	}
	return strings.Join(lines, "\n")
}
func renderHelmChartManifest(args InstallHelmChartArgs) string {
	namespace := strings.TrimSpace(args.Namespace)
	if namespace == "" {
		namespace = "kube-system"
	}
	lines := []string{"apiVersion: helm.cattle.io/v1", "kind: HelmChart", "metadata:", fmt.Sprintf("  name: %s", args.ReleaseName), "  namespace: kube-system", "spec:", fmt.Sprintf("  chart: %s", args.ChartSource)}
	if repo := strings.TrimSpace(args.Repo); repo != "" {
		lines = append(lines, fmt.Sprintf("  repo: %s", repo))
	}
	lines = append(lines, fmt.Sprintf("  targetNamespace: %s", namespace))
	if values := strings.TrimSpace(args.Values); values != "" {
		lines = append(lines, "  valuesContent: |")
		lines = append(lines, indentYAMLBlock(values))
	}
	return strings.Join(lines, "\n")
}
func HelmValuesYAML(raw any) string {
	switch v := raw.(type) {
	case string:
		return strings.TrimSpace(v)
	case map[string]any:
		b, err := json.MarshalIndent(v, "", "  ")
		if err != nil {
			return ""
		}
		return string(b)
	default:
		return ""
	}
}

type clusterAgentConfig struct {
	BridgeURL           string `json:"bridgeUrl"`
	BridgeMcpURL        string `json:"bridgeMcpUrl"`
	BridgeToken         string `json:"bridgeToken"`
	ClusterID           string `json:"clusterId"`
	ClusterName         string `json:"clusterName"`
	AgentID             string `json:"agentId"`
	APIEndpoint         string `json:"apiEndpoint,omitempty"`
	ProviderID          string `json:"providerId,omitempty"`
	ResourceID          string `json:"resourceId,omitempty"`
	PollIntervalSeconds int    `json:"pollIntervalSeconds"`
}

const clusterAgentServiceName = "opute-cluster-agent"

func renderClusterAgentServiceUnit() string {
	return fmt.Sprintf(`[Unit]
Description=Opute Cluster Agent
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=root
ExecStart=%s
Restart=always
RestartSec=5
Environment=OPUTE_AGENT_CONFIG=%s

[Install]
WantedBy=multi-user.target
`, "/usr/local/lib/opute/cluster-agent", "/etc/opute/cluster-agent.json")
}
func renderClusterAgentInstallScript(bridgeURL string, arch string, configJSON []byte) string {
	artifactURL := fmt.Sprintf("%s/artifacts/cluster-agent/%s.gz", strings.TrimRight(bridgeURL, "/"), arch)
	encodedConfig := base64.StdEncoding.EncodeToString(configJSON)
	encodedUnit := base64.StdEncoding.EncodeToString([]byte(renderClusterAgentServiceUnit()))
	return strings.Join([]string{
		"set -euo pipefail",
		"mkdir -p /usr/local/lib/opute /etc/opute",
		fmt.Sprintf("systemctl stop %s 2>/dev/null || true", clusterAgentServiceName),
		"if ! command -v curl >/dev/null 2>&1; then apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y curl; fi",
		"if ! command -v gunzip >/dev/null 2>&1; then apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y gzip; fi",
		fmt.Sprintf("curl -sfL --max-time %d %s | gunzip > %s", 180, ShellQuote(artifactURL), "/usr/local/lib/opute/cluster-agent"),
		fmt.Sprintf("chmod 755 %s", "/usr/local/lib/opute/cluster-agent"),
		fmt.Sprintf("printf '%%s' %s | base64 -d > %s", ShellQuote(encodedConfig), "/etc/opute/cluster-agent.json"),
		fmt.Sprintf("printf '%%s' %s | base64 -d > /etc/systemd/system/%s.service", ShellQuote(encodedUnit), clusterAgentServiceName),
		"systemctl daemon-reload",
		fmt.Sprintf("systemctl enable --now %s", clusterAgentServiceName),
		fmt.Sprintf("systemctl restart %s", clusterAgentServiceName),
	}, "\n")
}
func wrapHostPrivilegedShellBody(body string) string {
	escapedBody := strings.ReplaceAll(body, "'", "'\"'\"'")
	return strings.Join([]string{"set -euo pipefail", "if command -v sudo >/dev/null 2>&1 && sudo -n true 2>/dev/null; then", fmt.Sprintf("  sudo -n bash -lc '%s'", escapedBody), "elif [ \"$(id -u)\" -eq 0 ]; then", fmt.Sprintf("  bash -lc '%s'", escapedBody), "else", "  echo 'passwordless sudo is required to install the cluster agent on the host OS' >&2", "  exit 127", "fi"}, "\n")
}

func k8sAge(creationTimestamp string, now time.Time) string {
	if creationTimestamp == "" {
		return "unknown"
	}
	t, err := time.Parse(time.RFC3339, creationTimestamp)
	if err != nil {
		return "unknown"
	}
	elapsed := now.Sub(t)
	minutes := int(elapsed.Minutes())
	if minutes < 60 {
		return fmt.Sprintf("%dm", minutes)
	}
	hours := minutes / 60
	if hours < 48 {
		return fmt.Sprintf("%dh", hours)
	}
	return fmt.Sprintf("%dd", hours/24)
}

func main() {
	out := map[string]any{}
	out["chart_plain"] = renderHelmChartManifest(InstallHelmChartArgs{ReleaseName: "redis", ChartSource: "bitnami/redis"})
	out["chart_full"] = renderHelmChartManifest(InstallHelmChartArgs{ReleaseName: "redis", ChartSource: "redis", Namespace: " data ", Repo: " https://charts.example/x ", Values: "a: 1\nb:\n  c: <x>&\n\n"})
	out["values_map"] = HelmValuesYAML(map[string]any{"z": 1.0, "a": map[string]any{"b": "<&>", "c": []any{1.5, true, nil}}, "e": map[string]any{}, "f": []any{}})
	cfg, _ := json.Marshal(clusterAgentConfig{BridgeURL: "http://10.0.0.1:9093", BridgeMcpURL: "http://10.0.0.1:9093/mcp", BridgeToken: "t<o>k'en", ClusterID: "c1", ClusterName: "n", AgentID: "a", ProviderID: "p", PollIntervalSeconds: 5})
	out["config"] = string(cfg)
	out["script"] = renderClusterAgentInstallScript("http://10.0.0.1:9093/", "arm64", cfg)
	out["privileged"] = wrapHostPrivilegedShellBody(out["script"].(string))
	now := time.Date(2026, 10, 17, 12, 0, 0, 0, time.UTC)
	ages := map[string]string{}
	for _, ts := range []string{"", "bogus", "2026-10-17T11:59:30Z", "2026-10-17T11:00:00.5Z", "2026-10-17T10:00:00+01:00", "2026-10-15T12:00:01Z", "2026-10-10T00:00:00-07:00", "2026-10-17T12:30:00Z", "2026-02-30T00:00:00Z", "2026-10-17T11:00:00.123456789123Z", "2026-10-17t11:00:00Z", "2026-10-17T11:00:00z"} {
		ages[ts] = k8sAge(ts, now)
	}
	out["ages"] = ages
	b, _ := json.MarshalIndent(out, "", " ")
	fmt.Println(string(b))
}
