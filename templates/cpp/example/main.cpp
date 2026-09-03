// Ping-pong: the smallest program that exercises the whole interface.
//
// n0 pings the next node around the ring, which pongs back, for a fixed number of rounds. It uses
// every part of the template — handlers, send, set_timer with a callback, observe — and
// deliberately implements no algorithm from any lab.
//
//   c++ -std=c++17 -O2 -o example example/main.cpp
//   cuelight run --seed 1 --bin ./example
#include "../sdr.hpp"

int main() {
    sdr::Node node;
    int sent = 0;
    const int rounds = 5;

    auto other = [&]() -> std::string {
        for (size_t i = 0; i < node.peers.size(); ++i)
            if (node.peers[i] == node.id) return node.peers[(i + 1) % node.peers.size()];
        return node.id;
    };

    std::function<void()> ping = [&]() {
        if (sent >= rounds) return;
        sent++;
        sdr::Json m = sdr::Json::object();
        m["type"] = sdr::Json("ping");
        m["n"] = sdr::Json(sent);
        node.send(other(), m);
    };

    // Only the first node starts, so the ring carries exactly one token.
    node.on("init", [&](const std::string&, const sdr::Json&) {
        if (node.id == node.peers.front()) node.set_timer(10, ping);
    });
    node.on("ping", [&](const std::string& src, const sdr::Json& b) {
        sdr::Json o = sdr::Json::object();
        o["type"] = sdr::Json("saw_ping");
        o["n"] = sdr::Json(b.find("n")->as_int());
        o["from"] = sdr::Json(src);
        node.observe(o);
        sdr::Json p = sdr::Json::object();
        p["type"] = sdr::Json("pong");
        p["n"] = sdr::Json(b.find("n")->as_int());
        node.send(src, p);
    });
    node.on("pong", [&](const std::string&, const sdr::Json& b) {
        sdr::Json o = sdr::Json::object();
        o["type"] = sdr::Json("saw_pong");
        o["n"] = sdr::Json(b.find("n")->as_int());
        node.observe(o);
        node.set_timer(20, ping);
    });

    node.run();
}
