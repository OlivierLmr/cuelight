// Package cuelight is the node runtime: an event loop over JSON lines on stdin and stdout.
//
// A node is a *pure event handler*: it receives one event, emits its outgoing messages, and
// announces `done`. Same event sequence in, same actions out.
//
// Do not use wall-clock time, goroutines, or unseeded randomness. Replay depends on determinism,
// and the harness will detect and report violations as your bug. Your process must never exit.
package cuelight

import (
	"bufio"
	"encoding/json"
	"fmt"
	"os"
)

type Body map[string]any

type Handler func(src string, body Body)

type envelope struct {
	Src  string `json:"src"`
	Dest string `json:"dest"`
	Body Body   `json:"body"`
}

type Node struct {
	ID       string
	Peers    []string
	N, F     int
	Provided []string // layers the harness is standing in for

	handlers map[string]Handler
	timerCbs map[int]func()
	out      []envelope
	timerID  int
}

func New() *Node {
	return &Node{handlers: map[string]Handler{}, timerCbs: map[int]func(){}}
}

// On registers a handler for one message type.
func (n *Node) On(msgType string, h Handler) { n.handlers[msgType] = h }

func (n *Node) Send(dest string, b Body) {
	n.out = append(n.out, envelope{Src: n.ID, Dest: dest, Body: b})
}

// SetTimer fires after `after` units of LOGICAL time and returns the timer id.
//
// With a callback, only that callback runs when it fires, which is what lets several layers hold
// timers at once. Timers cannot be cancelled: if you re-arm, guard the callback with a generation
// counter and ignore stale firings.
func (n *Node) SetTimer(after int, cb func()) int {
	n.timerID++
	if cb != nil {
		n.timerCbs[n.timerID] = cb
	}
	n.Send("harness", Body{"type": "set_timer", "after": after, "timer_id": n.timerID})
	return n.timerID
}

// Observe reports an observable event (deliver / enter_cs / leader / decide / ...).
func (n *Node) Observe(b Body) { n.Send("harness", b) }

func (n *Node) Log(args ...any) { fmt.Fprintln(os.Stderr, args...) }

func (n *Node) Run() {
	in := bufio.NewScanner(os.Stdin)
	in.Buffer(make([]byte, 1<<20), 1<<20)
	w := bufio.NewWriter(os.Stdout)

	for in.Scan() {
		line := in.Bytes()
		if len(line) == 0 {
			continue
		}
		var m envelope
		if err := json.Unmarshal(line, &m); err != nil {
			continue
		}
		msgType, _ := m.Body["type"].(string)

		if msgType == "init" {
			n.ID, _ = m.Body["node_id"].(string)
			n.Peers = strings(m.Body["node_ids"])
			n.N, n.F = number(m.Body["n"]), number(m.Body["f"])
			n.Provided = strings(m.Body["provided"])
		}

		if msgType == "timer" {
			id := number(m.Body["timer_id"])
			if cb, ok := n.timerCbs[id]; ok {
				delete(n.timerCbs, id)
				cb()
			} else if h, ok := n.handlers["timer"]; ok {
				h(m.Src, m.Body)
			}
		} else if h, ok := n.handlers[msgType]; ok {
			h(m.Src, m.Body)
		}

		// `done` must be last: it is the barrier that lets logical time advance.
		n.Send("harness", Body{"type": "done"})
		for _, e := range n.out {
			b, _ := json.Marshal(e)
			w.Write(b)
			w.WriteByte('\n')
		}
		w.Flush()
		n.out = n.out[:0]
	}
}

// JSON numbers decode as float64 and arrays as []any; these keep the call sites readable.
func number(v any) int {
	if f, ok := v.(float64); ok {
		return int(f)
	}
	return 0
}

func strings(v any) []string {
	raw, ok := v.([]any)
	if !ok {
		return nil
	}
	out := make([]string, 0, len(raw))
	for _, x := range raw {
		if s, ok := x.(string); ok {
			out = append(out, s)
		}
	}
	return out
}
