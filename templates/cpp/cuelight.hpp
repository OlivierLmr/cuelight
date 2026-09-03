// Node runtime: an event loop over JSON lines on stdin and stdout. Header-only, no dependencies.
//
// A node is a *pure event handler*: it receives one event, emits its outgoing messages, and
// announces `done`. Same event sequence in, same actions out.
//
// Do not use wall-clock time, threads, or unseeded randomness — replay depends on determinism and
// the harness reports violations as your bug. Your process must never exit.
#pragma once

#include <functional>
#include <iostream>
#include <map>
#include <string>
#include <utility>
#include <vector>

namespace cuelight {

// ---------------------------------------------------------------- JSON ----
// Object keys keep insertion order, which keeps emitted lines stable and readable.
struct Json {
    enum class Type { Null, Bool, Num, Str, Arr, Obj };
    Type type = Type::Null;
    bool boolean = false;
    double number = 0;
    std::string str;
    std::vector<Json> arr;
    std::vector<std::pair<std::string, Json>> obj;

    Json() = default;
    Json(bool v) : type(Type::Bool), boolean(v) {}
    Json(double v) : type(Type::Num), number(v) {}
    Json(int v) : type(Type::Num), number(v) {}
    Json(const char* v) : type(Type::Str), str(v) {}
    Json(std::string v) : type(Type::Str), str(std::move(v)) {}

    static Json object() { Json j; j.type = Type::Obj; return j; }
    static Json array() { Json j; j.type = Type::Arr; return j; }

    bool has(const std::string& k) const { return find(k) != nullptr; }

    const Json* find(const std::string& k) const {
        for (auto& kv : obj) if (kv.first == k) return &kv.second;
        return nullptr;
    }

    Json& operator[](const std::string& k) {
        for (auto& kv : obj) if (kv.first == k) return kv.second;
        type = Type::Obj;
        obj.emplace_back(k, Json());
        return obj.back().second;
    }

    std::string as_str(const std::string& fallback = "") const {
        return type == Type::Str ? str : fallback;
    }
    int as_int(int fallback = 0) const {
        return type == Type::Num ? static_cast<int>(number) : fallback;
    }

    std::string dump() const {
        std::string o;
        write(o);
        return o;
    }

    void write(std::string& o) const {
        switch (type) {
            case Type::Null: o += "null"; break;
            case Type::Bool: o += boolean ? "true" : "false"; break;
            case Type::Num: {
                long long i = static_cast<long long>(number);
                o += (static_cast<double>(i) == number) ? std::to_string(i)
                                                        : std::to_string(number);
                break;
            }
            case Type::Str: quote(str, o); break;
            case Type::Arr: {
                o += '[';
                for (size_t i = 0; i < arr.size(); i++) {
                    if (i) o += ',';
                    arr[i].write(o);
                }
                o += ']';
                break;
            }
            case Type::Obj: {
                o += '{';
                for (size_t i = 0; i < obj.size(); i++) {
                    if (i) o += ',';
                    quote(obj[i].first, o);
                    o += ':';
                    obj[i].second.write(o);
                }
                o += '}';
                break;
            }
        }
    }

    static void quote(const std::string& s, std::string& o) {
        o += '"';
        for (char c : s) {
            switch (c) {
                case '"': o += "\\\""; break;
                case '\\': o += "\\\\"; break;
                case '\n': o += "\\n"; break;
                case '\r': o += "\\r"; break;
                case '\t': o += "\\t"; break;
                default: o += c;
            }
        }
        o += '"';
    }

    static Json parse(const std::string& s) {
        size_t i = 0;
        return parse_value(s, i);
    }

private:
    static void ws(const std::string& s, size_t& i) {
        while (i < s.size() && isspace(static_cast<unsigned char>(s[i]))) i++;
    }

    static Json parse_value(const std::string& s, size_t& i) {
        ws(s, i);
        if (i >= s.size()) return Json();
        switch (s[i]) {
            case '{': return parse_obj(s, i);
            case '[': return parse_arr(s, i);
            case '"': return Json(parse_str(s, i));
            case 't': i += 4; return Json(true);
            case 'f': i += 5; return Json(false);
            case 'n': i += 4; return Json();
            default: return parse_num(s, i);
        }
    }

    static Json parse_obj(const std::string& s, size_t& i) {
        Json j = object();
        i++; ws(s, i);
        if (s[i] == '}') { i++; return j; }
        while (true) {
            ws(s, i);
            std::string k = parse_str(s, i);
            ws(s, i); i++;                       // ':'
            j.obj.emplace_back(k, parse_value(s, i));
            ws(s, i);
            if (s[i] == ',') { i++; continue; }
            i++; return j;                       // '}'
        }
    }

    static Json parse_arr(const std::string& s, size_t& i) {
        Json j = array();
        i++; ws(s, i);
        if (s[i] == ']') { i++; return j; }
        while (true) {
            j.arr.push_back(parse_value(s, i));
            ws(s, i);
            if (s[i] == ',') { i++; continue; }
            i++; return j;                       // ']'
        }
    }

    static std::string parse_str(const std::string& s, size_t& i) {
        std::string o;
        i++;                                     // opening quote
        while (i < s.size()) {
            char c = s[i++];
            if (c == '"') break;
            if (c != '\\') { o += c; continue; }
            char e = s[i++];
            switch (e) {
                case 'n': o += '\n'; break;
                case 't': o += '\t'; break;
                case 'r': o += '\r'; break;
                case 'u': i += 4; o += '?'; break;
                default: o += e;
            }
        }
        return o;
    }

    static Json parse_num(const std::string& s, size_t& i) {
        size_t start = i;
        while (i < s.size() && (isdigit(static_cast<unsigned char>(s[i])) ||
                                s[i] == '-' || s[i] == '+' || s[i] == '.' ||
                                s[i] == 'e' || s[i] == 'E')) i++;
        return Json(std::stod(s.substr(start, i - start)));
    }
};

// ---------------------------------------------------------------- Node ----
class Node {
public:
    std::string id;
    std::vector<std::string> peers;
    int n = 0, f = 0;
    std::vector<std::string> provided;      // layers the harness stands in for

    using Handler = std::function<void(const std::string&, const Json&)>;

    void on(const std::string& type, Handler h) { handlers_[type] = std::move(h); }

    void send(const std::string& dest, Json body) {
        Json e = Json::object();
        e["src"] = Json(id);
        e["dest"] = Json(dest);
        e["body"] = std::move(body);
        out_.push_back(std::move(e));
    }

    // Fires after `after` units of LOGICAL time. With a callback, only that callback runs, which
    // is what lets several layers hold timers at once. Timers cannot be cancelled: if you re-arm,
    // guard the callback with a generation counter and ignore stale firings.
    int set_timer(int after, std::function<void()> cb = nullptr) {
        timer_id_++;
        if (cb) timer_cbs_[timer_id_] = std::move(cb);
        Json b = Json::object();
        b["type"] = Json("set_timer");
        b["after"] = Json(after);
        b["timer_id"] = Json(timer_id_);
        send("harness", std::move(b));
        return timer_id_;
    }

    // Report an observable event (deliver / enter_cs / leader / decide / ...).
    void observe(Json body) { send("harness", std::move(body)); }

    template <typename... A> void log(A&&... a) { (std::cerr << ... << a) << std::endl; }

    void run() {
        std::string line;
        while (std::getline(std::cin, line)) {
            if (line.empty()) continue;
            Json m = Json::parse(line);
            const Json* bp = m.find("body");
            if (!bp) continue;
            const Json& b = *bp;
            std::string src = m.find("src") ? m.find("src")->as_str() : "";
            std::string type = b.find("type") ? b.find("type")->as_str() : "";

            if (type == "init") {
                id = b.find("node_id")->as_str();
                peers.clear();
                for (auto& x : b.find("node_ids")->arr) peers.push_back(x.as_str());
                n = b.find("n")->as_int();
                f = b.find("f")->as_int();
                provided.clear();
                if (const Json* p = b.find("provided"))
                    for (auto& x : p->arr) provided.push_back(x.as_str());
            }

            if (type == "timer") {
                int tid = b.find("timer_id") ? b.find("timer_id")->as_int() : 0;
                auto it = timer_cbs_.find(tid);
                if (it != timer_cbs_.end()) {
                    auto cb = it->second;
                    timer_cbs_.erase(it);
                    cb();
                } else if (handlers_.count("timer")) {
                    handlers_["timer"](src, b);
                }
            } else if (handlers_.count(type)) {
                handlers_[type](src, b);
            }

            // `done` must be last: it is the barrier that lets logical time advance.
            Json d = Json::object();
            d["type"] = Json("done");
            send("harness", std::move(d));
            for (auto& e : out_) std::cout << e.dump() << "\n";
            std::cout.flush();
            out_.clear();
        }
    }

private:
    std::map<std::string, Handler> handlers_;
    std::map<int, std::function<void()>> timer_cbs_;
    std::vector<Json> out_;
    int timer_id_ = 0;
};

}  // namespace cuelight
