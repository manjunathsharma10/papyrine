#!/usr/bin/env python3
"""Spike 0.4: download public forms into corpus/cache/js-forms/ (gitignored).

Usage: python3 fetch.py   (idempotent; skips files already present)
Writes corpus/cache/js-forms/_fetch.json: {id: {url, source, license, sha256, size}}
"""
import hashlib, json, os, re, subprocess, sys, urllib.request, urllib.error
from concurrent.futures import ThreadPoolExecutor

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
OUT = os.path.join(ROOT, "corpus", "cache", "js-forms")
os.makedirs(OUT, exist_ok=True)
UA = "Mozilla/5.0 (compatible; papyrine-corpus-spike/0.1; +https://github.com/manjunathsharma10/papyrine)"
MAX = 40 * 1024 * 1024

cands = []  # (source, id, url, license)

def add(source, lic, base, names, ext=".pdf", pre=""):
    for n in names:
        cands.append((source, f"{source}-{pre}{n}".lower().replace("/", "_"), base.format(n=n) if "{n}" in base else base + n + ext, lic))

US = "public-domain (US federal government work)"
irs = """f1040 f1040sc f1040sd f1040se f1040sa f1040sb f1040s1 f1040s2 f1040s3 f1040es f1040x f1040nr f1040v f1040sr f1040sse f1040sf f1040sh f1040sei f1040s8 f2441 f2106 f3800 f4562 f4868 f5329 f6251 f8283 f8829 f8949 f8863 f8812 f8995 f8995a f8962 f8606 f8582 f8880 f8889 f8888 f8867 f2848 f8821 f8822 f8822b f4506 f4506t f433a f433b f656 f9465 f1098 f1098e f1098t f1099int f1099div f1099misc f1099nec f1099r f1099g f1099k f1099b f1099s f1099a f1099c f1099q f1099sa f1096 fw2 fw3 fw4 fw4p fw8ben fw8bene fw8eci fw8imy fw9 fw7 f941 f940 f944 f943 f945 f941x f1065 f1120 f1120s f1065sk1 f1120ssk f1041 f1128 f2553 f8832 f990 f990ez f990pf f990t f1023 f1024 f5500 f5498 f4684 f4797 f4952 f6252 f8824 f2210 f2555 f1116 f1118 f5471 f5472 f8938 f8858 f8865 f3520 f8300 f8594 f8615 f8814 f8839 f8853 f8857 f8379 f8396 f8453 f8892 f1310 f2290 f720 f730 f11c f637 f8821 f13844 f14039 f14653 f13614c f8508 f8809 f8809i f1099h f1099ltc f1099oid f1099patr f1099ls f3949a f4137 f4070 f8919 f8959 f8960 f2439 f4972 f5695 f8396 f3903 f8801 f6765 f3468""".split()
add("irs", US, "https://www.irs.gov/pub/irs-pdf/", irs)

opm = """sf86 sf50 sf52 sf61 sf85 sf85p sf87 sf15 sf144 sf180 sf2809 sf2810 sf2817 sf3107 sf3108 sf3112a sf2823 sf1152 sf2800 sf2801 sf2802 sf1150 sf3104 sf3105 sf3109 sf2803 sf2804 sf2818 sf2819 sf2821 sf2822 sf2824 sf1153 sf3102 sf2808 sf2806 sf2807 sf3101 sf75 sf8 sf50b sf113a sf1126 sf1190 sf1199a sf3112b sf3112c sf3112d sf3112e sf3103 sf3106 sf3110 sf3111 sf3113 sf3114 sf1187 sf2812 sf2811 sf3107 sf9 sf2813 sf3101a sf3105a sf86c sf85pc sf50-b opm71 opm1203 opm1203fx opm1010 opm1496 opm1612 opm1655 opm1331 opm1325 opm1496a opm1496b opm1203a opm1325 opm1326 opm1327 opm1328 opm1329 opm1330 opm1332 opm1333 opm1334 opm1335 opm1336""".split()
add("opm", US, "https://www.opm.gov/forms/pdf_fill/", sorted(set(opm)))

va = """VBA-21-526EZ-ARE VBA-21-0966-ARE VBA-21-4138-ARE VBA-21-686c-ARE VBA-21-22-ARE VBA-21-0781-ARE VBA-21-4142-ARE VBA-21P-527EZ-ARE VBA-21P-534EZ-ARE VBA-21-674-ARE VBA-22-1990-ARE VBA-22-1995-ARE VBA-22-5490-ARE VBA-21-0845-ARE VBA-21-0972-ARE VBA-21-4142a-ARE VBA-21-8940-ARE VBA-21-10210-ARE VBA-20-0995-ARE VBA-20-0996-ARE VBA-10182-ARE VBA-21-0960M-1-ARE VBA-21-2680-ARE VBA-21-4502-ARE VBA-26-1880-ARE VBA-26-1817-ARE VBA-29-336-ARE VBA-21-0304-ARE VBA-21-4170-ARE VBA-21-4192-ARE VBA-21-4140-ARE VBA-21-4165-ARE""".split()
add("va", US, "https://www.vba.va.gov/pubs/forms/", va)
va2 = "VA10-10EZ VA10-10CG VA10-10EZR VA10-10172 VA10-0137 VA10-5345 VA10-3542 VA10-7959C VA10-10162 VA10-2850 VA10-10068".split()
add("va", US, "https://www.va.gov/vaforms/va/pdf/", va2)

uscis = """i-9 i-90 i-130 i-131 i-485 i-765 i-864 n-400 n-600 i-129 i-140 i-539 i-751 i-821 g-28 g-639 i-693 i-824 i-912 i-134 ar-11 i-129f i-130a i-131a i-360 i-290b i-526 i-589 i-601 i-601a i-751 i-765v i-817 i-854 i-864a i-864ez i-865 i-907 i-918 i-929 n-336 n-426 n-565 n-648 g-325a g-845 g-884 g-1145 g-1256 i-102 i-212 i-246 i-356 i-407 i-508 i-566 i-600 i-600a i-693 i-800 i-800a""".split()
add("uscis", US, "https://www.uscis.gov/sites/default/files/document/forms/", sorted(set(uscis)))

ssa = """ssa-7004 ssa-5 ssa-3288 ssa-827 ssa-561 ssa-89 ssa-44 ssa-795 ssa-3441 ssa-16 ssa-1372 ssa-3380 ssa-1 ssa-2 ssa-4 ssa-7 ssa-8 ssa-10 ssa-454 ssa-3368 ssa-3369 ssa-3373 ssa-1696 ssa-1199 ssa-1294 ssa-7008 ssa-632 ssa-7157 ssa-L996 ssa-1010 ssa-8001 ssa-1020 ssa-3820""".split()
add("ssa", US, "https://www.ssa.gov/forms/", ssa)

dod = """dd0214 dd1351-2 dd2058 dd0149 dd0577 dd2875 dd1750 dd0093 dd0214 dd0058 dd0067 dd0137-3 dd0173 dd0214 dd0368 dd0398 dd0577 dd1056 dd1172-2 dd1348-1a dd1387 dd1610 dd2062 dd2282 dd2345 dd2365 dd2560 dd2657 dd2765 dd2793 dd2822 dd2870 dd2873 dd2977 dd1556 dd2501 dd1966 dd1380 dd2808 dd2807-1 dd2807-2 dd2808""".split()
add("dod", US, "https://www.esd.whs.mil/Portals/54/Documents/DD/forms/dd/", sorted(set(dod)))

state = """ds11 ds82 ds5504 ds64 ds160 ds2029 ds3053 ds3052 ds5505 ds71 ds7002 ds60 ds234 ds3032 ds4079""".split()
add("state-dept", US, "https://eforms.state.gov/Forms/", state)

misc = [
 ("hud","https://www.hud.gov/sites/documents/52646.PDF"),("hud","https://www.hud.gov/sites/documents/92900A.PDF"),("hud","https://www.hud.gov/sites/documents/92900-LT.PDF"),("hud","https://www.hud.gov/sites/documents/52517.PDF"),("hud","https://www.hud.gov/sites/documents/50058.PDF"),("hud","https://www.hud.gov/sites/documents/DOC_12066.PDF"),("hud","https://www.hud.gov/sites/documents/HUD903.PDF"),("hud","https://www.hud.gov/sites/documents/1003.PDF"),
 ("cms","https://www.cms.gov/Medicare/CMS-Forms/CMS-Forms/downloads/cms855i.pdf"),("cms","https://www.cms.gov/Medicare/CMS-Forms/CMS-Forms/downloads/cms855a.pdf"),("cms","https://www.cms.gov/Medicare/CMS-Forms/CMS-Forms/downloads/cms855b.pdf"),("cms","https://www.cms.gov/Medicare/CMS-Forms/CMS-Forms/downloads/cms855r.pdf"),("cms","https://www.cms.gov/Medicare/CMS-Forms/CMS-Forms/downloads/cms10114.pdf"),("cms","https://www.cms.gov/Medicare/CMS-Forms/CMS-Forms/downloads/cms40b.pdf"),("cms","https://www.cms.gov/Medicare/CMS-Forms/CMS-Forms/downloads/cms1696.pdf"),("cms","https://www.cms.gov/Medicare/CMS-Forms/CMS-Forms/downloads/cms460.pdf"),("cms","https://www.cms.gov/Medicare/CMS-Forms/CMS-Forms/downloads/cms10106.pdf"),("cms","https://www.cms.gov/Medicare/CMS-Forms/CMS-Forms/downloads/cms855s.pdf"),("cms","https://www.cms.gov/Medicare/CMS-Forms/CMS-Forms/downloads/cms588.pdf"),
 ("dol","https://www.dol.gov/sites/dolgov/files/WHD/legacy/files/wh380e.pdf"),("dol","https://www.dol.gov/sites/dolgov/files/WHD/legacy/files/wh381.pdf"),("dol","https://www.dol.gov/sites/dolgov/files/WHD/legacy/files/wh380f.pdf"),("dol","https://www.dol.gov/sites/dolgov/files/WHD/legacy/files/WH-226.pdf"),("dol","https://www.dol.gov/sites/dolgov/files/OWCP/regs/compliance/ca-1.pdf"),("dol","https://www.dol.gov/sites/dolgov/files/OWCP/regs/compliance/CA-2.pdf"),("dol","https://www.dol.gov/sites/dolgov/files/OWCP/regs/compliance/CA-7.pdf"),("dol","https://www.dol.gov/sites/dolgov/files/OWCP/regs/compliance/CA-16.pdf"),("dol","https://www.dol.gov/sites/dolgov/files/ebsa/employers-and-advisers/plan-administration-and-compliance/reporting-and-filing/form-5500/2023-form-5500.pdf"),
 ("usps","https://about.usps.com/forms/ps1583.pdf"),("usps","https://about.usps.com/forms/ps3575.pdf"),("usps","https://about.usps.com/forms/ps3811.pdf"),("usps","https://about.usps.com/forms/ps1000.pdf"),("usps","https://about.usps.com/forms/ps2976.pdf"),("usps","https://about.usps.com/forms/ps3801.pdf"),("usps","https://about.usps.com/forms/ps3541.pdf"),
 ("fema","https://www.fema.gov/sites/default/files/documents/fema_form-086-0-9_standard-flood-hazard-determination.pdf"),("fema","https://www.fema.gov/sites/default/files/documents/fema_form-ff-206-fy-21-100_elevation-certificate.pdf"),("fema","https://www.fema.gov/pdf/nfip/manual202110/content/20_flood_proof.pdf"),
 ("fda","https://www.fda.gov/media/76798/download"),("fda","https://www.fda.gov/media/76299/download"),("fda","https://www.fda.gov/media/72466/download"),("fda","https://www.fda.gov/media/70040/download"),("fda","https://www.fda.gov/media/72348/download"),
 ("sba","https://www.sba.gov/sites/default/files/2023-03/SBAForm1919_0.pdf"),("sba","https://www.sba.gov/sites/default/files/2023-09/SBA%20Form%201244.pdf"),("sba","https://www.sba.gov/sites/default/files/2021-01/SBA%20Form%20413.pdf"),("sba","https://www.sba.gov/sites/default/files/2022-11/SBA%20Form%20912.pdf"),
 ("eeoc","https://www.eeoc.gov/sites/default/files/migrated_files/employees/eeoc-form5.pdf"),
 ("faa","https://www.faa.gov/forms/index.cfm/download/8710-1"),("faa","https://www.faa.gov/documentLibrary/media/Form/FAA_Form_8500-8.pdf"),
 ("atf","https://www.atf.gov/file/61446/download"),("atf","https://www.atf.gov/file/61581/download"),("atf","https://www.atf.gov/file/60511/download"),("atf","https://www.atf.gov/file/61551/download"),
 ("treasury","https://www.fiscal.treasury.gov/files/forms/sf1199a.pdf"),("treasury","https://www.fiscal.treasury.gov/files/forms/sf3881.pdf"),("treasury","https://www.fiscal.treasury.gov/files/forms/fs-form-1200.pdf"),
 ("gsa","https://www.gsa.gov/system/files/SF30-19.pdf"),("gsa","https://www.gsa.gov/system/files/SF1449-18.pdf"),("gsa","https://www.gsa.gov/system/files/SF26-18.pdf"),("gsa","https://www.gsa.gov/system/files/SF33-18.pdf"),("gsa","https://www.gsa.gov/system/files/SF1034-18.pdf"),("gsa","https://www.gsa.gov/system/files/SF1408.pdf"),("gsa","https://www.gsa.gov/system/files/SF1094.pdf"),("gsa","https://www.gsa.gov/system/files/SF1164.pdf"),("gsa","https://www.gsa.gov/system/files/SF298.pdf"),("gsa","https://www.gsa.gov/system/files/SF182.pdf"),("gsa","https://www.gsa.gov/system/files/SF1012-18.pdf"),("gsa","https://www.gsa.gov/system/files/SF1034-11.pdf"),
 ("uscourts","https://www.uscourts.gov/sites/default/files/form_b101_0.pdf"),("uscourts","https://www.uscourts.gov/sites/default/files/b_106sum_0624.pdf"),("uscourts","https://www.uscourts.gov/sites/default/files/b_107_0624.pdf"),("uscourts","https://www.uscourts.gov/sites/default/files/b_122a-1_0624.pdf"),("uscourts","https://www.uscourts.gov/sites/default/files/b_121_0624.pdf"),("uscourts","https://www.uscourts.gov/sites/default/files/ao240.pdf"),("uscourts","https://www.uscourts.gov/sites/default/files/ao398.pdf"),("uscourts","https://www.uscourts.gov/sites/default/files/ao085.pdf"),("uscourts","https://www.uscourts.gov/sites/default/files/ao440.pdf"),("uscourts","https://www.uscourts.gov/sites/default/files/ao239.pdf"),("uscourts","https://www.uscourts.gov/sites/default/files/ao241.pdf"),("uscourts","https://www.uscourts.gov/sites/default/files/ao242.pdf"),("uscourts","https://www.uscourts.gov/sites/default/files/ao243.pdf"),("uscourts","https://www.uscourts.gov/sites/default/files/ao91.pdf"),("uscourts","https://www.uscourts.gov/sites/default/files/ao88b.pdf"),
 ("fincen","https://www.fincen.gov/sites/default/files/shared/FinCEN%20Form%20114_FBAR.pdf"),
 ("nrc","https://www.nrc.gov/docs/ML0129/ML012910218.pdf"),
 ("ed-fsa","https://studentaid.gov/sites/default/files/fsawg/datacenter/library/DirectLoanMPN.pdf"),
 ("dhs","https://www.cbp.gov/sites/default/files/assets/documents/2020-Feb/CBP-Form-I-94.pdf"),("dhs","https://www.ice.gov/doclib/sevis/pdf/i20.pdf"),
]
for s, u in misc:
    nm = re.sub(r"[^a-z0-9]+", "-", u.split("/")[-1].lower().rsplit(".pdf", 1)[0]) or "f"
    if nm == "download": nm = re.sub(r"[^a-z0-9]+", "-", u.split("/")[-2].lower())
    cands.append((s, f"{s}-{nm}", u, US))

# US states
ST = "government publication (US state); not redistributed"
states = [
 ("ny","https://www.tax.ny.gov/pdf/current_forms/it/{n}_fill_in.pdf", "it201 it203 it204 it2105 it216 it214 it225 it227 it180 it182 it196 it112r it113 it558 it2658 it2663 it399 it611 it641 it215 it180 it40 it209 it250 it280 it2106 it245 it370 it195 it601 it602 it603 it604 it605 it606 it607 it638 it639".split()),
 ("ny","https://www.tax.ny.gov/pdf/current_forms/st/{n}_fill_in.pdf", "st100 st101 st102 st108 st119_1 st121 st124 st125 st810 st830 st809".split()),
 ("ca","https://www.ftb.ca.gov/forms/2023/2023-{n}.pdf", "540 540nr 3532 3514 3506 3519 3521 3522 3523 3800 3805e 3809 3885a 5805 540-2ez 541 565 568 100 100s 109 3801 3803 3840 3853 3885 5870a 592 593 3893 3592 3548 3560 3582 3583 3584 3586 3587 3588 3589".split()),
 ("ca","https://edd.ca.gov/pdf_pub_ctr/{n}.pdf", "de4 de1 de1rw de2 de34 de542 de4p de2088 de38 de1378 de131 de429d de88 de9 de9c de24 de16 de2503 de2501 de1545 de2511 de8714".split()),
 ("fl","https://floridarevenue.com/Forms_library/current/{n}.pdf", "dr15 dr15n dr15z dr1 dr1n dr15cs dr15ez dr14 dr26 dr312 dr501 dr225 dr156 dr601 dr15mo dr15mo dr7 dr1nd dr5 dr26sb dr703 dr5ev dr309".split()),
 ("il","https://tax.illinois.gov/content/dam/soi/en/web/tax/forms/incometax/documents/currentyear/individual/{n}.pdf", "il-1040 il-1040-x schedule-cr il-8453 il-1040-es il-4852 il-w-4 schedule-nr schedule-m schedule-1299-c schedule-ic schedule-3 sch-ncb".split()),
 ("pa","https://www.revenue.pa.gov/FormsandPublications/FormsforIndividuals/PIT/Documents/2023/2023_{n}.pdf", "pa-40 pa-40_in pa-40_es pa-40_sp pa-40_w pa-40_d pa-40_c pa-40_t pa-40_u".split()),
 ("nj","https://nj.gov/treasury/taxation/pdf/current/{n}.pdf", "nj1040 nj1040nr nj-w4 nj1040es nj-wr nj1065 nj1080c nj500 nj630 nj1040x".split()),
 ("tx","https://comptroller.texas.gov/forms/{n}.pdf", "05-102 05-158-a 05-158-b 05-163 05-165 05-359 05-390 01-114 01-339 01-340 01-346 01-917 01-922 01-923 02-101 12-301 12-302 12-303 12-304 96-259 96-1014 01-122 01-128 01-143 01-157 05-143 05-144 05-145 05-147 05-149 05-151 05-152".split()),
 ("va-state","https://www.tax.virginia.gov/sites/default/files/vatax-pdf/{n}.pdf", "2023-760 2023-760py 2023-760-fill 2023-schedule-adj 2023-760f 2023-760c 2023-763 2023-ar1 va-4 va-5 va-6".split()),
 ("wa","https://dor.wa.gov/sites/default/files/{n}.pdf", "".split()),
 ("or","https://www.oregon.gov/dor/forms/FormsPubs/{n}.pdf", "form-or-40_101-040_2023 form-or-40-v_150-101-043_2023 form-or-40-p_150-101-046_2023 form-or-40-n_150-101-045_2023 form-or-w-4_150-101-402".split()),
 ("co","https://tax.colorado.gov/sites/tax/files/documents/{n}.pdf", "DR0104 DR0104AD DR0104CH DR0104PN DR0158-I DR0900 DR1778 DR0252 DR0078 DR0108".split()),
 ("mn","https://www.revenue.state.mn.us/sites/default/files/{n}.pdf", "2023-m1 2023-m1m 2023-m1pr 2023-m1ni 2023-m1w 2023-m3 2023-m4".split()),
 ("ma","https://www.mass.gov/doc/{n}/download", "2023-form-1/ 2023-form-1-nr/ 2023-form-m-4868".split()),
 ("ga","https://dor.georgia.gov/document/document/{n}/download", "".split()),
 ("oh","https://tax.ohio.gov/static/forms/ohio_individual/individual/{n}.pdf", "2023/it1040 2023/sd100 2023/it1040es 2023/it4708 2023/it1041 2023/it4738 2023/it4567 2023/it2210 2023/it2023 2023/it1040-sch-a 2023/it1040-sch-b".split()),
 ("mi","https://www.michigan.gov/taxes/-/media/Project/Websites/taxes/Forms/{n}.pdf", "".split()),
 ("wi","https://www.revenue.wi.gov/DOR%20Forms/{n}.pdf", "".split()),
 ("nc","https://www.ncdor.gov/documents/files/{n}.pdf", "d-400-2023 d-400-schedule-s-2023 nc-4 nc-4ez nc-5 nc-3 d-400tc-2023 d-400-schedule-pn-2023 d-401-2023 cd-405-2023".split()),
 ("az","https://azdor.gov/sites/default/files/document/FORMS/{n}.pdf", "".split()),
 ("ky","https://revenue.ky.gov/Forms/{n}.pdf", "".split()),
 ("md","https://www.marylandtaxes.gov/forms/{n}.pdf", "".split()),
 ("ok","https://oklahoma.gov/content/dam/ok/en/tax/documents/forms/individuals/current/{n}.pdf", "511 511-nr 538-s 512-ez 511-cr 561 538-h 511-v 511-ez".split()),
 ("ut","https://tax.utah.gov/forms/current/{n}.pdf", "tc-40 tc-40a tc-40b tc-40w tc-40e tc-40s tc-69 tc-61 tc-20 tc-65 tc-41 tc-40f".split()),
 ("ia","https://tax.iowa.gov/sites/default/files/2023-12/{n}.pdf", "".split()),
]
for src, base, names in states:
    for n in names:
        url = base.replace("{n}", n)
        cands.append((src, f"{src}-{re.sub(r'[^a-z0-9]+','-',n.lower()).strip('-')}-{hashlib.md5(url.encode()).hexdigest()[:4]}", url, ST))

# Canada / Australia / others
CA = "Crown copyright (Government of Canada); not redistributed"
ircc = "imm5257e imm1294e imm5707e imm0008e imm5710e imm5708e imm5709e imm1295e imm5476e imm5532e imm5645e imm5406e imm5444e imm5562e imm5669e imm5409e imm5409 imm5483e imm5257b imm5489e imm1442e imm5535e imm5368e imm5373e imm5401e imm5513e imm5523e imm5604e imm5605e imm5606e imm5618e imm5619e imm5620e imm5621e imm5622e imm5623e imm5624e imm5625e imm5626e imm5627e imm5628e imm5629e imm5630e imm5631e imm5632e imm5633e imm5634e imm5635e imm5636e imm5637e imm5638e imm5639e imm5640e imm5641e imm5642e imm5643e imm5644e imm5646e imm5647e imm5648e imm5649e imm5650e imm5651e imm5652e imm5653e imm5654e imm5655e imm5656e imm5657e imm5658e imm5659e imm5660e imm5661e imm5662e imm5663e imm5664e imm5665e imm5666e imm5667e imm5668e imm5670e imm5671e imm5672e imm5673e imm5674e imm5675e imm5676e imm5677e imm5678e imm5679e imm5680e imm5681e imm5682e imm5683e imm5684e imm5685e imm5686e imm5687e imm5688e imm5689e imm5690e imm5691e imm5692e imm5693e imm5694e imm5695e imm5696e imm5697e imm5698e imm5699e imm5700e imm5701e imm5702e imm5703e imm5704e imm5705e imm5706e imm5711e imm5712e imm5713e imm5714e imm5715e imm5716e imm5717e imm5718e imm5719e imm5720e imm5721e imm5722e imm5723e imm5724e imm5725e imm5726e imm5727e imm5728e imm5729e imm5730e".split()
# keep realistic subset: known real ones first, then a handful of probes
for n in ircc[:40]:
    cands.append(("ircc", f"ircc-{n}", f"https://www.canada.ca/content/dam/ircc/migration/ircc/english/pdf/kits/forms/{n}.pdf", CA))
for n in "td1-fill-24e t2200-fill-23e t1213-fill-23e rc4288-fill-23e t2201-fill-23e t1-2023-fill-23e t4032-fill rc66 rc7 rc59 gst20 gst10 t2091 t1255 t1135-fill-23e t2125-fill-23e t777-fill-23e t1ts rc4100 t4012 t2 t3 t1-adj rc1 rc243 t2057 t2058 t5013 tp-1 t2050 t2054 t2062 t2064 t2062a".split():
    cands.append(("cra", f"cra-{n}", f"https://www.canada.ca/content/dam/cra-arc/formspubs/pbg/{n.split('-')[0]}/{n}.pdf", CA))
AU = "Commonwealth of Australia copyright; not redistributed"
for n in "1221 47a 80 1229 956 157a 1419 1008 1022 1026 1163 1401 1402 1404 1405 1406 1407 1408 1409 1410 1411 1412 1413 1414 1415 1416 1417 1418 1420 1421 1422 1423 1424 1425 1426 1427 1428 1429 1430 1431 1432 1433 1434 1435 1436 1437 1438 1439 1440 1441 1442 1443 1444 1445".split()[:18]:
    cands.append(("homeaffairs", f"homeaffairs-{n}", f"https://immi.homeaffairs.gov.au/form-listing/forms/{n}.pdf", AU))
for n in "nat2681 nat2432 nat3092 nat2677 nat7216 nat1432 nat2678 nat2679 nat2680 nat2683 nat2684 nat2685".split():
    cands.append(("ato", f"ato-{n}", f"https://www.ato.gov.au/uploadedFiles/Content/IND/Downloads/{n}.pdf", AU))
OGL = "Open Government Licence v3.0 / Crown copyright"

def get(url):
    r = subprocess.run(["curl", "-sSL", "--max-time", "90", "--max-filesize", str(MAX), "-A", UA, "-f", url],
                       capture_output=True)
    if r.returncode != 0:
        raise RuntimeError("curl %d %s" % (r.returncode, r.stderr.decode()[:60]))
    return r.stdout

def fetch(c):
    source, id_, url, lic = c
    path = os.path.join(OUT, id_ + ".pdf")
    if os.path.exists(path):
        data = open(path, "rb").read()
    else:
        try:
            data = get(url)
        except Exception as e:
            return (c, None, str(e)[:80])
        if len(data) > MAX or b"%PDF" not in data[:1024]:
            return (c, None, "not a pdf / too big")
        open(path, "wb").write(data)
    return (c, hashlib.sha256(data).hexdigest(), len(data))

# GOV.UK, Canada.ca etc. discovered via APIs
def govuk():
    import urllib.parse
    out = []
    seen = set()
    for start in range(0, 1200, 100):
        try:
            u = "https://www.gov.uk/api/search.json?filter_content_store_document_type=form&count=100&start=%d&fields=link" % start
            res = json.loads(get(u))["results"]
        except Exception:
            break
        for r in res:
            try:
                ct = json.loads(get("https://www.gov.uk/api/content" + r["link"]))
            except Exception:
                continue
            for a in ct.get("details", {}).get("attachments", []):
                url = a.get("url", "")
                if url.lower().endswith(".pdf") and url not in seen:
                    seen.add(url)
                    out.append(("gov-uk", "govuk-" + re.sub(r"[^a-z0-9]+", "-", url.split("/")[-1].lower()[:-4])[:50] + "-" + hashlib.md5(url.encode()).hexdigest()[:4], url, OGL))
        if len(res) < 100:
            break
        if len(out) > 300: break
    return out

def github_tree(repo, prefix, source, lic, limit, idpre):
    try:
        t = json.loads(subprocess.check_output(["gh", "api", f"repos/{repo}/git/trees/HEAD?recursive=1"]))
    except Exception as e:
        print("gh fail", repo, e, file=sys.stderr); return []
    out = []
    for e in t["tree"]:
        if e["path"].startswith(prefix) and e["path"].lower().endswith(".pdf") and e.get("size", 0) < 8_000_000:
            n = e["path"].split("/")[-1][:-4]
            out.append((source, f"{idpre}-{re.sub(r'[^a-z0-9]+','-',n.lower())[:60]}", f"https://raw.githubusercontent.com/{repo}/HEAD/{e['path']}", lic))
    return out[:limit]

if __name__ == "__main__":
    mode = sys.argv[1] if len(sys.argv) > 1 else "all"
    extra = []
    if mode in ("all", "ext"):
        extra += github_tree("mozilla/pdf.js", "test/pdfs/", "pdfjs-corpus", "Apache-2.0 (pdf.js test corpus; individual files vary)", 400, "pdfjs")
        extra += govuk()
    allc = cands + extra if mode != "ext" else extra
    seen = set(); uniq = []
    for c in allc:
        if c[1] in seen: continue
        seen.add(c[1]); uniq.append(c)
    print("candidates", len(uniq), file=sys.stderr)
    meta_path = os.path.join(OUT, "_fetch.json")
    meta = json.load(open(meta_path)) if os.path.exists(meta_path) else {}
    ok = 0
    with ThreadPoolExecutor(12) as ex:
        for c, sha, info in ex.map(fetch, uniq):
            if sha:
                ok += 1
                meta[c[1]] = {"url": c[2], "source": c[0], "license": c[3], "sha256": sha, "size": info}
            else:
                print("FAIL", c[1], info, file=sys.stderr)
    json.dump(meta, open(meta_path, "w"), indent=1)
    print("downloaded ok", ok, "total meta", len(meta), file=sys.stderr)
