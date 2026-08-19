// Example node: reliable broadcast, to check the template end to end.
#include "../sdr.hpp"
#include <set>

int main() {
    sdr::Node node;
    std::set<std::string> seen;
    int seq = 0;

    auto accept = [&](const sdr::Json& m) {
        std::string mid = m.find("mid")->as_str();
        if (!seen.insert(mid).second) return;
        node.broadcast(m);                                   // relay BEFORE delivering
        sdr::Json d = sdr::Json::object();
        d["type"] = sdr::Json("deliver");
        d["mid"] = sdr::Json(m.find("payload")->find("mid")->as_str());
        node.observe(d);
    };

    node.on("do_broadcast", [&](const std::string&, const sdr::Json& b) {
        seq++;
        sdr::Json payload = sdr::Json::object();
        payload["mid"] = sdr::Json(b.find("mid")->as_str());
        sdr::Json m = sdr::Json::object();
        m["type"] = sdr::Json("rb");
        m["mid"] = sdr::Json(node.id + "-" + std::to_string(seq));
        m["payload"] = payload;
        accept(m);
    });
    node.on("rb", [&](const std::string&, const sdr::Json& b) { accept(b); });

    node.run();
}
