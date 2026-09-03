// Ping-pong: the smallest program that exercises the whole interface.
//
// n0 pings the next node around the ring, which pongs back, for a fixed number of rounds. It uses
// every part of the template: handlers, Send, SetTimer with a callback, Observe. It deliberately
// implements no algorithm from any lab.
//
//	go build -o example ./example && cuelight run --seed 1 --bin ./example
package main

import "cuelight"

const rounds = 5

func main() {
	node := cuelight.New()
	sent := 0

	other := func() string {
		for i, p := range node.Peers {
			if p == node.ID {
				return node.Peers[(i+1)%len(node.Peers)]
			}
		}
		return node.ID
	}

	var ping func()
	ping = func() {
		if sent >= rounds {
			return
		}
		sent++
		node.Send(other(), cuelight.Body{"type": "ping", "n": sent})
	}

	// Only the first node starts, so the ring carries exactly one token.
	node.On("init", func(src string, b cuelight.Body) {
		if node.ID == node.Peers[0] {
			node.SetTimer(10, ping)
		}
	})
	node.On("ping", func(src string, b cuelight.Body) {
		node.Observe(cuelight.Body{"type": "saw_ping", "n": b["n"], "from": src})
		node.Send(src, cuelight.Body{"type": "pong", "n": b["n"]})
	})
	node.On("pong", func(src string, b cuelight.Body) {
		node.Observe(cuelight.Body{"type": "saw_pong", "n": b["n"]})
		node.SetTimer(20, ping)
	})

	node.Run()
}
