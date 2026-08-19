// Example node: reliable broadcast, to check the template end to end.
package main

import "sdr"

func main() {
	node := sdr.New()
	seen := map[string]bool{}
	seq := 0

	deliver := func(payload sdr.Body) {
		node.Observe(sdr.Body{"type": "deliver", "mid": payload["mid"]})
	}
	accept := func(m sdr.Body) {
		mid, _ := m["mid"].(string)
		if seen[mid] {
			return
		}
		seen[mid] = true
		node.Broadcast(m)                       // relay BEFORE delivering
		deliver(m["payload"].(map[string]any))
	}

	node.On("do_broadcast", func(src string, b sdr.Body) {
		seq++
		accept(sdr.Body{
			"type":    "rb",
			"mid":     node.ID + "-" + string(rune('0'+seq)),
			"payload": map[string]any{"mid": b["mid"]},
		})
	})
	node.On("rb", func(src string, b sdr.Body) { accept(b) })

	node.Run()
}
