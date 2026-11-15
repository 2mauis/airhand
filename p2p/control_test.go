package p2p

import (
	"bytes"
	"encoding/json"
	"strings"
	"testing"
)

// A malformed command must close the control session without echoing input.
func TestControlRejectsUnknownCommand(t *testing.T) {
	var out, diagnostics bytes.Buffer
	err := RunControl(strings.NewReader("{\"cmd\":\"unknown\",\"address\":\"sensitive-example\"}\n"), &out, &diagnostics)
	if err == nil {
		t.Fatal("unknown control command accepted")
	}
	if strings.Contains(out.String()+diagnostics.String()+err.Error(), "sensitive-example") {
		t.Fatal("control command echoed")
	}
	var event Event
	if json.Unmarshal(bytes.TrimSpace(out.Bytes()), &event) != nil || event.Kind != "error" || event.Path != "none" {
		t.Fatal("missing closed error event")
	}
}

func TestHelperArgumentsAreFixed(t *testing.T) {
	args := HelperArgv()
	if len(args) != 1 || args[0] != "--control" {
		t.Fatal("helper must use the fixed control entry point")
	}
}
