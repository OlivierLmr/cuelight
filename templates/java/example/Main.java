// Ping-pong: the smallest program that exercises the whole interface.
//
// n0 pings the next node around the ring, which pongs back, for a fixed number of rounds. It uses
// every part of the template — handlers, send, setTimer with a callback, observe — and deliberately
// implements no algorithm from any lab.
//
//   javac -d out sdr/*.java example/Main.java
//   cuelight run --seed 1 --bin java -cp out Main
import sdr.Node;

public class Main {
    static final int ROUNDS = 5;
    static int sent = 0;
    static Node node = new Node();

    static String other() {
        int i = node.peers.indexOf(node.id);
        return node.peers.get((i + 1) % node.peers.size());
    }

    static void ping() {
        if (sent >= ROUNDS) return;
        sent++;
        node.send(other(), Node.body("type", "ping", "n", sent));
    }

    public static void main(String[] args) throws Exception {
        // Only the first node starts, so the ring carries exactly one token.
        node.on("init", (src, b) -> {
            if (node.id.equals(node.peers.get(0))) node.setTimer(10, Main::ping);
        });
        node.on("ping", (src, b) -> {
            node.observe(Node.body("type", "saw_ping", "n", b.get("n"), "from", src));
            node.send(src, Node.body("type", "pong", "n", b.get("n")));
        });
        node.on("pong", (src, b) -> {
            node.observe(Node.body("type", "saw_pong", "n", b.get("n")));
            node.setTimer(20, Main::ping);
        });
        node.run();
    }
}
