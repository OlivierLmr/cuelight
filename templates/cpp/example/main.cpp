// Ping-pong: the smallest program that exercises the whole interface.
//
// n0 pings the next node around the ring, which pongs back, for a fixed number of rounds. It uses
// every part of the template — handlers, send, set_timer with a callback, observe — and
// deliberately implements no algorithm from any lab.
//
//   c++ -std=c++17 -O2 -o example example/main.cpp
//   cuelight run --seed 1 --bin ./example
#include "../cuelight.hpp"

int main() {
    cuelight::Node node;
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
        cuelight::Json m = cuelight::Json::object();
        m["type"] = cuelight::Json("ping");
        m["n"] = cuelight::Json(sent);
        node.send(other(), m);
    };

    // Only the first node starts, so the ring carries exactly one token.
    node.on("init", [&](const std::string&, const cuelight::Json&) {
        if (node.id == node.peers.front()) node.set_timer(10, ping);
    });
    node.on("ping", [&](const std::string& src, const cuelight::Json& b) {
        cuelight::Json o = cuelight::Json::object();
        o["type"] = cuelight::Json("saw_ping");
        o["n"] = cuelight::Json(b.find("n")->as_int());
        o["from"] = cuelight::Json(src);
        node.observe(o);
        cuelight::Json p = cuelight::Json::object();
        p["type"] = cuelight::Json("pong");
        p["n"] = cuelight::Json(b.find("n")->as_int());
        node.send(src, p);
    });
    node.on("pong", [&](const std::string&, const cuelight::Json& b) {
        cuelight::Json o = cuelight::Json::object();
        o["type"] = cuelight::Json("saw_pong");
        o["n"] = cuelight::Json(b.find("n")->as_int());
        node.observe(o);
        node.set_timer(20, ping);
    });

    node.run();
}
