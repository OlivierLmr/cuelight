// Example node: reliable broadcast, to check the template end to end.
import java.util.HashMap;
import java.util.HashSet;
import java.util.Map;
import java.util.Set;
import sdr.Node;

public class Main {
    static final Set<String> seen = new HashSet<>();
    static int seq = 0;

    public static void main(String[] args) throws Exception {
        Node node = new Node();

        node.on("do_broadcast", (src, b) -> {
            seq++;
            Map<String, Object> payload = Node.body("mid", b.get("mid"));
            accept(node, Node.body("type", "rb", "mid", node.id + "-" + seq, "payload", payload));
        });
        node.on("rb", (src, b) -> accept(node, b));

        node.run();
    }

    @SuppressWarnings("unchecked")
    static void accept(Node node, Map<String, Object> m) {
        String mid = (String) m.get("mid");
        if (!seen.add(mid)) return;
        node.broadcast(m);                                  // relay BEFORE delivering
        Map<String, Object> payload = (Map<String, Object>) m.get("payload");
        node.observe(Node.body("type", "deliver", "mid", payload.get("mid")));
    }
}
