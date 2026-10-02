// Coarse helper operations over qpdf's document-helper classes: AcroForm, annotation flattening,
// page copying with fields, outlines, page labels, embedded files, name/number trees and lazy
// stream data. Everything here is reachable only through cxx and is wrapped by `trycatch`.
#include "papyrine_shim.h"
#include "qpdf-sys/src/lib.rs.h"

#include <qpdf/Constants.h>
#include <qpdf/QPDFAcroFormDocumentHelper.hh>
#include <qpdf/QPDFAnnotationObjectHelper.hh>
#include <qpdf/QPDFEFStreamObjectHelper.hh>
#include <qpdf/QPDFEmbeddedFileDocumentHelper.hh>
#include <qpdf/QPDFFileSpecObjectHelper.hh>
#include <qpdf/QPDFFormFieldObjectHelper.hh>
#include <qpdf/QPDFNameTreeObjectHelper.hh>
#include <qpdf/QPDFNumberTreeObjectHelper.hh>
#include <qpdf/QPDFOutlineDocumentHelper.hh>
#include <qpdf/QPDFOutlineObjectHelper.hh>
#include <qpdf/QPDFPageDocumentHelper.hh>
#include <qpdf/QPDFPageLabelDocumentHelper.hh>
#include <qpdf/QPDFPageObjectHelper.hh>
#include <qpdf/Pipeline.hh>

#include <algorithm>
#include <map>
#include <set>

namespace papyrine {

namespace {

rust::Vec<uint8_t> bytes_of(std::string const& s)
{
    rust::Vec<uint8_t> v;
    v.reserve(s.size());
    for (unsigned char c : s) {
        v.push_back(c);
    }
    return v;
}

std::string str_of(rust::Slice<uint8_t const> s)
{
    return std::string(reinterpret_cast<char const*>(s.data()), s.size());
}

std::unique_ptr<Obj> wrap_obj(QPDFObjectHandle h)
{
    return std::make_unique<Obj>(Obj{std::move(h)});
}

QPDFObjectHandle checked_dict(Doc const& d, int32_t id, int32_t gen)
{
    QPDFObjectHandle h = d.q->getObject(id, gen);
    if (!h.isDictionary()) {
        throw ShimError(kTypeError, "type error: expected field dictionary");
    }
    return h;
}

int field_kind(QPDFFormFieldObjectHelper& f)
{
    if (f.isText()) {
        return 1;
    }
    if (f.isCheckbox()) {
        return 2;
    }
    if (f.isRadioButton()) {
        return 3;
    }
    if (f.isPushbutton()) {
        return 4;
    }
    if (f.isChoice()) {
        return 5;
    }
    if (f.getFieldType() == "/Sig") {
        return 6;
    }
    return 0;
}

// Name or string -> bytes (without a leading slash for names); empty otherwise.
std::string name_or_string(QPDFObjectHandle const& h)
{
    if (h.isName()) {
        std::string n = h.getName();
        return n.size() && n[0] == '/' ? n.substr(1) : n;
    }
    if (h.isString()) {
        return h.getUTF8Value();
    }
    return {};
}

} // namespace

// ---------------------------------------------------------------------------------------------
// Lazy stream data

void stream_replace_provider(Obj const& o, rust::Box<ProviderBox> provider, Obj const& filter,
                             Obj const& decode_parms)
{
    if (!o.h.isStream()) {
        throw ShimError(kTypeError, "type error: expected stream");
    }
    // std::function must be copyable, so the Rust box lives behind a shared_ptr.
    auto holder = std::make_shared<rust::Box<ProviderBox>>(std::move(provider));
    QPDFObjectHandle h = o.h;
    h.replaceStreamData(
        [holder](Pipeline* p) {
            // A Rust error surfaces as a C++ exception (rust::Error) and is mapped by trycatch.
            rust::Vec<uint8_t> data = provider_produce(**holder);
            if (!data.empty()) {
                p->write(data.data(), data.size());
            }
            p->finish();
        },
        filter.h, decode_parms.h);
}

// ---------------------------------------------------------------------------------------------
// AcroForm

bool form_has_acroform(Doc const& d)
{
    return d.q->getRoot().getKey("/AcroForm").isDictionary();
}

rust::Vec<FieldInfo> form_fields(Doc const& d)
{
    rust::Vec<FieldInfo> out;
    if (!form_has_acroform(d)) {
        return out;
    }
    QPDFAcroFormDocumentHelper afdh(*d.q);

    // Page index of every widget annotation.
    std::map<QPDFObjGen, int32_t> widget_page;
    auto const& pages = d.q->getAllPages();
    for (size_t i = 0; i < pages.size(); ++i) {
        for (auto& a : afdh.getWidgetAnnotationsForPage(QPDFPageObjectHelper(pages[i]))) {
            widget_page.emplace(a.getObjectHandle().getObjGen(), static_cast<int32_t>(i));
        }
    }

    for (auto& f : afdh.getFormFields()) {
        FieldInfo fi{};
        QPDFObjGen og = f.getObjectHandle().getObjGen();
        fi.id = og.getObj();
        fi.gen_ = og.getGen();
        fi.kind = field_kind(f);
        fi.flags = f.getFlags();
        fi.quadding = f.getQuadding();
        fi.checked = fi.kind == 2 && f.isChecked();
        QPDFObjectHandle ml = f.getInheritableFieldValue("/MaxLen");
        fi.max_len = ml.isInteger() ? ml.getIntValueAsInt() : -1;
        fi.field_type = bytes_of(f.getFieldType());
        fi.name = bytes_of(f.getFullyQualifiedName());
        fi.partial_name = bytes_of(f.getPartialName());
        fi.alt_name = bytes_of(f.getAlternativeName());
        std::string v = f.getValueAsString();
        if (v.empty()) {
            v = name_or_string(f.getValue());
        }
        fi.value = bytes_of(v);
        std::string dv = f.getDefaultValueAsString();
        if (dv.empty()) {
            dv = name_or_string(f.getDefaultValue());
        }
        fi.default_value = bytes_of(dv);
        fi.default_appearance = bytes_of(f.getDefaultAppearance());
        if (fi.kind == 5) {
            for (auto const& c : f.getChoices()) {
                fi.choices.push_back(Bytes{bytes_of(c)});
            }
        }
        std::set<std::string> states;
        for (auto& a : afdh.getAnnotationsForField(f)) {
            WidgetInfo wi{};
            QPDFObjGen aog = a.getObjectHandle().getObjGen();
            wi.id = aog.getObj();
            wi.gen_ = aog.getGen();
            auto it = widget_page.find(aog);
            wi.page = it == widget_page.end() ? -1 : it->second;
            auto r = a.getRect();
            wi.x0 = r.llx;
            wi.y0 = r.lly;
            wi.x1 = r.urx;
            wi.y1 = r.ury;
            fi.widgets.push_back(wi);
            if (fi.kind == 2 || fi.kind == 3) {
                QPDFObjectHandle n = a.getAppearanceDictionary().getKey("/N");
                if (n.isDictionary()) {
                    for (auto const& k : n.getKeys()) {
                        if (k != "/Off") {
                            states.insert(k.substr(1));
                        }
                    }
                }
            }
        }
        for (auto const& s : states) {
            fi.states.push_back(Bytes{bytes_of(s)});
        }
        out.push_back(std::move(fi));
    }
    return out;
}

void form_set_value(Doc const& d, int32_t id, int32_t gen, rust::Slice<uint8_t const> value,
                    bool appearance)
{
    QPDFFormFieldObjectHelper f(checked_dict(d, id, gen));
    std::string v = str_of(value);
    int kind = field_kind(f);
    if (kind == 2 || kind == 3) {
        if (v.empty() || v[0] != '/') {
            v.insert(v.begin(), '/');
        }
        // qpdf maps the requested state to the widgets' real on/off states and sets /AS.
        f.setV(QPDFObjectHandle::newName(v), false);
    } else if (kind == 1 || kind == 5) {
        f.setV(v, false);
        if (appearance) {
            QPDFAcroFormDocumentHelper afdh(*d.q);
            for (auto& a : afdh.getAnnotationsForField(f)) {
                f.generateAppearance(a);
            }
        }
    } else {
        throw ShimError(3, "unsupported: only text, choice, checkbox and radio fields can be set");
    }
}

bool form_need_appearances(Doc const& d)
{
    if (!form_has_acroform(d)) {
        return false;
    }
    return QPDFAcroFormDocumentHelper(*d.q).getNeedAppearances();
}

void form_set_need_appearances(Doc const& d, bool v)
{
    if (!form_has_acroform(d)) {
        return;
    }
    QPDFAcroFormDocumentHelper(*d.q).setNeedAppearances(v);
}

void form_generate_appearances(Doc const& d)
{
    if (!form_has_acroform(d)) {
        return;
    }
    QPDFAcroFormDocumentHelper afdh(*d.q);
    afdh.setNeedAppearances(true);
    afdh.generateAppearancesIfNeeded();
    afdh.setNeedAppearances(false);
}

// ---------------------------------------------------------------------------------------------
// Pages

void flatten_annotations(Doc const& d, int32_t required_flags, int32_t forbidden_flags)
{
    QPDFPageDocumentHelper(*d.q).flattenAnnotations(required_flags, forbidden_flags);
}

void remove_unreferenced_resources(Doc const& d)
{
    QPDFPageDocumentHelper(*d.q).removeUnreferencedResources();
}

void page_remove_unreferenced_resources(Doc const& d, Obj const& page)
{
    (void)d;
    if (!page.h.isDictionary()) {
        throw ShimError(kTypeError, "type error: expected page dictionary");
    }
    QPDFPageObjectHelper(page.h).removeUnreferencedResources();
}

rust::Vec<ObjGenPair> copy_pages(Doc const& d, Doc const& src, rust::Slice<uint32_t const> indices,
                                 size_t at)
{
    rust::Vec<ObjGenPair> out;
    bool same = d.q.get() == src.q.get();
    if (!same) {
        // addPage does this for foreign pages; keep the source's inherited attributes local.
        src.q->pushInheritedAttributesToPage();
    }
    std::vector<QPDFObjectHandle> from_pages = src.q->getAllPages();
    for (uint32_t i : indices) {
        if (i >= from_pages.size()) {
            throw ShimError(kRangeError, "source page index out of range");
        }
    }
    QPDFAcroFormDocumentHelper dst_afdh(*d.q);
    std::unique_ptr<QPDFAcroFormDocumentHelper> own_src;
    if (!same) {
        own_src = std::make_unique<QPDFAcroFormDocumentHelper>(*src.q);
    }
    QPDFAcroFormDocumentHelper& src_afdh = same ? dst_afdh : *own_src;

    size_t pos = std::min(at, d.q->getAllPages().size());
    for (uint32_t i : indices) {
        QPDFObjectHandle from = from_pages[i];
        size_t count = d.q->getAllPages().size();
        if (pos >= count) {
            d.q->addPage(from, false);
            pos = count;
        } else {
            QPDFObjectHandle ref = d.q->getAllPages()[pos];
            d.q->addPageAt(from, true, ref);
        }
        QPDFObjectHandle added = d.q->getAllPages()[pos];
        dst_afdh.fixCopiedAnnotations(added, from, src_afdh);
        QPDFObjGen og = added.getObjGen();
        out.push_back(ObjGenPair{og.getObj(), og.getGen()});
        ++pos;
    }
    return out;
}

// ---------------------------------------------------------------------------------------------
// Outlines, page labels, embedded files

namespace {

void walk_outline(Doc const& d, QPDFOutlineObjectHelper o, int depth, rust::Vec<OutlineItem>& out)
{
    constexpr size_t kMaxItems = 200000;
    if (out.size() >= kMaxItems) {
        return;
    }
    OutlineItem it{};
    QPDFObjectHandle h = o.getObjectHandle();
    it.id = h.getObjectID();
    it.gen_ = h.getGeneration();
    it.depth = depth;
    it.count = o.getCount();
    it.title = bytes_of(o.getTitle());
    it.page = -1;
    QPDFObjectHandle dp = o.getDestPage();
    if (dp.isDictionary()) {
        try {
            it.page = d.q->findPage(dp);
        } catch (std::exception const&) {
            it.page = -1;
        }
    }
    QPDFObjectHandle dest = h.getKey("/Dest");
    QPDFObjectHandle act = h.getKey("/A");
    if (act.isDictionary()) {
        QPDFObjectHandle s = act.getKey("/S");
        if (s.isName() && s.getName() == "/URI" && act.getKey("/URI").isString()) {
            it.uri = bytes_of(act.getKey("/URI").getUTF8Value());
        } else if (s.isName() && s.getName() == "/GoTo") {
            dest = act.getKey("/D");
        }
    }
    if (dest.isName() || dest.isString()) {
        it.dest_name = bytes_of(name_or_string(dest));
    }
    out.push_back(std::move(it));
    for (auto& k : o.getKids()) {
        walk_outline(d, k, depth + 1, out);
    }
}

} // namespace

rust::Vec<OutlineItem> outlines_read(Doc const& d)
{
    rust::Vec<OutlineItem> out;
    QPDFOutlineDocumentHelper oh(*d.q);
    if (!oh.hasOutlines()) {
        return out;
    }
    for (auto& top : oh.getTopLevelOutlines()) {
        walk_outline(d, top, 0, out);
    }
    return out;
}

rust::Vec<LabelRange> page_labels_read(Doc const& d)
{
    rust::Vec<LabelRange> out;
    QPDFObjectHandle root = d.q->getRoot().getKey("/PageLabels");
    if (!root.isDictionary()) {
        return out;
    }
    QPDFNumberTreeObjectHelper nt(root, *d.q);
    for (auto const& kv : nt.getAsMap()) {
        QPDFObjectHandle v = kv.second;
        if (!v.isDictionary()) {
            continue;
        }
        LabelRange r{};
        r.start_page = static_cast<int32_t>(std::clamp<long long>(kv.first, 0, INT32_MAX));
        r.style = 0;
        QPDFObjectHandle s = v.getKey("/S");
        if (s.isName()) {
            std::string n = s.getName();
            r.style = n == "/D" ? 1 : n == "/a" ? 2 : n == "/A" ? 3 : n == "/r" ? 4 : n == "/R" ? 5 : 0;
        }
        QPDFObjectHandle p = v.getKey("/P");
        if (p.isString()) {
            r.prefix = bytes_of(p.getUTF8Value());
        }
        QPDFObjectHandle st = v.getKey("/St");
        r.first_value = st.isInteger() ? st.getIntValueAsInt() : 1;
        out.push_back(std::move(r));
    }
    return out;
}

void page_labels_write(Doc const& d, rust::Slice<LabelRange const> ranges)
{
    QPDFObjectHandle cat = d.q->getRoot();
    if (ranges.empty()) {
        cat.removeKey("/PageLabels");
        return;
    }
    auto nt = QPDFNumberTreeObjectHelper::newEmpty(*d.q);
    for (auto const& r : ranges) {
        QPDFObjectHandle dict = QPDFObjectHandle::newDictionary();
        static char const* const styles[] = {nullptr, "/D", "/a", "/A", "/r", "/R"};
        if (r.style >= 1 && r.style <= 5) {
            dict.replaceKey("/S", QPDFObjectHandle::newName(styles[r.style]));
        }
        if (!r.prefix.empty()) {
            dict.replaceKey("/P", QPDFObjectHandle::newUnicodeString(std::string(
                                      reinterpret_cast<char const*>(r.prefix.data()),
                                      r.prefix.size())));
        }
        if (r.first_value != 1) {
            dict.replaceKey("/St", QPDFObjectHandle::newInteger(r.first_value));
        }
        nt.insert(r.start_page, dict);
    }
    cat.replaceKey("/PageLabels", nt.getObjectHandle());
}

rust::Vec<EmbeddedFileInfo> embedded_files_list(Doc const& d)
{
    rust::Vec<EmbeddedFileInfo> out;
    QPDFEmbeddedFileDocumentHelper eh(*d.q);
    if (!eh.hasEmbeddedFiles()) {
        return out;
    }
    for (auto const& kv : eh.getEmbeddedFiles()) {
        EmbeddedFileInfo e{};
        e.name = bytes_of(kv.first);
        e.size = -1;
        auto fs = kv.second;
        e.filename = bytes_of(fs->getFilename());
        e.description = bytes_of(fs->getDescription());
        QPDFObjectHandle spec = fs->getObjectHandle();
        e.filespec_id = spec.getObjectID();
        e.filespec_gen = spec.getGeneration();
        QPDFObjectHandle stream = fs->getEmbeddedFileStream();
        if (stream.isStream()) {
            e.stream_id = stream.getObjectID();
            e.stream_gen = stream.getGeneration();
            QPDFEFStreamObjectHelper ef(stream);
            std::string sub = ef.getSubtype();
            e.mime = bytes_of(sub.size() && sub[0] == '/' ? sub.substr(1) : sub);
            e.created = bytes_of(ef.getCreationDate());
            e.modified = bytes_of(ef.getModDate());
            QPDFObjectHandle params = stream.getDict().getKey("/Params");
            if (params.isDictionary()) {
                QPDFObjectHandle sz = params.getKey("/Size");
                if (sz.isInteger()) {
                    e.size = sz.getIntValue();
                }
                QPDFObjectHandle cs = params.getKey("/CheckSum");
                if (cs.isString()) {
                    e.checksum = bytes_of(cs.getStringValue());
                }
            }
        }
        out.push_back(std::move(e));
    }
    return out;
}

// ---------------------------------------------------------------------------------------------
// Name and number trees

std::unique_ptr<Obj> name_tree_new(Doc const& d)
{
    return wrap_obj(QPDFNameTreeObjectHelper::newEmpty(*d.q).getObjectHandle());
}

rust::Vec<Bytes> name_tree_keys(Doc const& d, Obj const& root)
{
    rust::Vec<Bytes> out;
    QPDFNameTreeObjectHelper nt(root.h, *d.q);
    for (auto const& kv : nt.getAsMap()) {
        out.push_back(Bytes{bytes_of(kv.first)});
    }
    return out;
}

std::unique_ptr<Obj> name_tree_get(Doc const& d, Obj const& root, rust::Slice<uint8_t const> key)
{
    QPDFNameTreeObjectHelper nt(root.h, *d.q);
    QPDFObjectHandle v;
    if (nt.findObject(str_of(key), v)) {
        return wrap_obj(v);
    }
    return nullptr;
}

void name_tree_set(Doc const& d, Obj const& root, rust::Slice<uint8_t const> key, Obj const& value)
{
    QPDFNameTreeObjectHelper nt(root.h, *d.q);
    std::string k = str_of(key);
    nt.remove(k);
    nt.insert(k, value.h);
}

bool name_tree_remove(Doc const& d, Obj const& root, rust::Slice<uint8_t const> key)
{
    QPDFNameTreeObjectHelper nt(root.h, *d.q);
    return nt.remove(str_of(key));
}

std::unique_ptr<Obj> number_tree_new(Doc const& d)
{
    return wrap_obj(QPDFNumberTreeObjectHelper::newEmpty(*d.q).getObjectHandle());
}

rust::Vec<int64_t> number_tree_keys(Doc const& d, Obj const& root)
{
    rust::Vec<int64_t> out;
    QPDFNumberTreeObjectHelper nt(root.h, *d.q);
    for (auto const& kv : nt.getAsMap()) {
        out.push_back(kv.first);
    }
    return out;
}

std::unique_ptr<Obj> number_tree_get(Doc const& d, Obj const& root, int64_t key)
{
    QPDFNumberTreeObjectHelper nt(root.h, *d.q);
    QPDFObjectHandle v;
    if (nt.findObject(key, v)) {
        return wrap_obj(v);
    }
    return nullptr;
}

void number_tree_set(Doc const& d, Obj const& root, int64_t key, Obj const& value)
{
    QPDFNumberTreeObjectHelper nt(root.h, *d.q);
    nt.remove(key);
    nt.insert(key, value.h);
}

bool number_tree_remove(Doc const& d, Obj const& root, int64_t key)
{
    QPDFNumberTreeObjectHelper nt(root.h, *d.q);
    return nt.remove(key);
}

} // namespace papyrine
