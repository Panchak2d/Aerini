# Aerini Commercial License Agreement

**Version 1.0**

This Aerini Commercial License Agreement ("Agreement") is between Panchaketu Debbarma, maintainer of the Aerini project ("Licensor"), and the individual or organization identified on the applicable License Certificate ("Licensee"). This Agreement takes effect on the date Licensee completes payment through Licensor's designated checkout process ("Effective Date") and applies only to the Licensed Version identified on the Certificate.

Licensor's designated checkout process is currently operated through Polar (polar.sh), acting solely as payment processor and merchant of record. Polar is not a party to this Agreement.

By completing checkout and checking the acceptance box presented at that time, Licensee agrees to be bound by this Agreement. That acceptance in electronic form is valid and enforceable under Section 10A of the Information Technology Act, 2000 (India), as amended, and under any corresponding law recognising contracts formed by electronic means that is applicable to Licensee. If Licensee does not agree, Licensee must not complete the purchase, and may continue to use Aerini under AGPL-3.0 instead.

---

## 1. Definitions

**"Software"** means Aerini, the visual workflow automation application (including `aerini-engine`, `aerini-server`, and its associated desktop and CLI components), as made available by Licensor. It does not include third-party dependencies bundled with or used by Aerini, which remain governed by their own licenses.

**"Licensed Version"** means the version of the Software identified on the Certificate, together with any patch-level release Licensor issues within the same minor-version line (for example, a Certificate naming 0.4 covers every subsequent 0.4.z patch). A new minor or major release (for example, 0.5.0 or 1.0.0) is not covered and requires a new Certificate.

**"Modifications"** means any change, addition, or derivative work Licensee makes to the Software.

**"Licensee"** means the individual or organization named on the Certificate. Where an organization is named, "Licensee" includes that organization's employees and contractors acting on its behalf for its own business, and excludes any parent, subsidiary, or affiliate not itself named, unless the Certificate states otherwise.

**"Embedded Product"** means the single product or service of Licensee's, identified by name on the Certificate, into which Licensee embeds the Licensed Version, or which Licensee operates using the Licensed Version, under Sections 3(b) and 3(c).

**"Certificate"** means the record of Licensee's purchase that Licensor's checkout process generates and delivers automatically when payment completes. It consists of: (i) the License ID, which is the license key the checkout process issues to Licensee; (ii) the Licensed Version, as named in the product description shown at checkout; (iii) the Embedded Product, as entered by Licensee in the checkout form; (iv) Licensee's identity, as entered by Licensee at checkout; and (v) the purchase date and fee, as shown on the order confirmation or receipt. The Certificate is delivered through the checkout process's order confirmation, receipt, and customer portal, and is not a separate signed document. The Certificate is evidence of the license; Licensee's rights under this Agreement arise on the Effective Date, not on issuance of the Certificate.

**"AI/ML System"** and **"Training Data"** have the meanings given to them in Section 1 of the Aerini Contributor License Agreement, version 1.4. If a later version of the Contributor License Agreement changes those definitions, the definitions in version 1.4 continue to apply to this Agreement unless Licensor and Licensee agree otherwise.

---

## 2. Relationship to AGPL-3.0

The Software is also available under AGPL-3.0. This Agreement is an alternative to AGPL-3.0, not an addition to it: for as long as this Agreement remains in force, it governs Licensee's use of the Licensed Version, to the extent of the rights granted in Section 3, in place of AGPL-3.0's source-availability obligations generally — including the network-copyleft requirement in Section 13 and the source-disclosure obligations that apply to distribution. Any other AGPL-3.0 permission Licensee already had continues to apply to the extent it does not conflict with this Agreement. Use outside the rights granted in Section 3 remains governed by AGPL-3.0.

---

## 3. Grant of License

Subject to Licensee's compliance with this Agreement and payment of the fee stated on the Certificate, Licensor grants Licensee a perpetual (as to the Licensed Version only — see Section 1), non-exclusive, non-transferable (see Section 9) license to:

**(a)** use, reproduce, and modify the Licensed Version for Licensee's own internal purposes and as part of Licensee's own products or services;

**(b)** distribute the Licensed Version, including Modifications, in object-code or source-code form, to Licensee's own end customers as part of the Embedded Product, without AGPL-3.0's obligation to publish source code or Modifications, and sublicense to those end customers the limited right to use the object-code form as part of the Embedded Product, subject to the flow-down requirement in Section 5; and

**(c)** operate a modified `aerini-server` as a network-accessible service for others, as part of the Embedded Product, without triggering AGPL-3.0 Section 13.

Sections 3(b) and 3(c) apply to the Embedded Product only, however many end customers or users it has. Each additional product or service Licensee wishes to distribute or operate under Section 3(b) or 3(c) requires a separate Certificate and fee. Section 3(a) is not limited to the Embedded Product, but does not extend to distribution, or to network-accessible service for others, which Sections 3(b) and 3(c) alone govern.

This license covers the Licensed Version only and does not extend to any later version of the Software.

---

## 4. Restrictions

Licensee may not:

**(a)** remove or obscure any copyright, license, or attribution notice contained in the Software;

**(b)** state or imply that Licensee's product or service is produced or endorsed by Licensor, or that it is "Aerini" rather than a product built on Aerini (see Section 8);

**(c)** license or sublicense the Software itself, or a rebrand of it, as a standalone competing product. Distributing the Licensed Version as a component of the Embedded Product, which Section 3(b) permits, is not a standalone competing product if both of the following are true: (i) the Embedded Product has materially different functionality, branding, or customer relationship from the unmodified Software; and (ii) the Embedded Product is not marketed primarily as a way to obtain the Software itself;

**(d)** use any part of the Software as Training Data for an AI/ML System (see Section 5); or

**(e)** rely on Section 3(b) or 3(c) for any product or service other than the Embedded Product, unless a separate Certificate names it.

---

## 5. AI/ML Training Restriction

Aerini's contributors license their contributions to Licensor on the condition that those contributions are never used as Training Data for an AI/ML System. Licensor is contractually required to pass that restriction on to every sublicensee, without exception.

Licensee — and anyone Licensee further distributes the Software to — may not use the Software, in whole or in part, as Training Data for any AI/ML System: to train, fine-tune, pre-train, distil, or otherwise update the learned parameters or weights of such a system, or to build a dataset for that purpose. This does not prohibit ordinary developer tooling that does not update model weights (code completion, static analysis, search indexing). See Licensor's published AI and Machine Learning Policy for the full scope.

**Flow-Down.** Licensee must include a contractually binding restatement of this Section 5, or a restriction at least as protective, in every end-user agreement under which Licensee distributes the Licensed Version under Section 3(b). Licensee has no right, under this Agreement or any end-user agreement Licensee enters into, to authorise the excluded use described in this Section 5. Licensee's failure to include the required restatement in an end-user agreement under Section 3(b) is itself a breach of this Section 5.

This Section survives termination of this Agreement and cannot be waived by any other provision of this Agreement or any later agreement between the parties, unless that later agreement amends this Section by explicit reference to it.

---

## 6. Ownership

The Software, including all contributions incorporated into it, remains the property of Licensor and Aerini's contributors. This Agreement grants a license, not a sale or assignment of intellectual property. Licensee owns its own original Modifications, subject to Licensor's underlying rights in the Software those Modifications are built on.

---

## 7. No Runtime Enforcement

The Software does not phone home, validate a license key, or technically enforce this Agreement. Licensee's right to use the Software under this Agreement rather than AGPL-3.0 depends on Licensee holding a current license for the Licensed Version — meaning this Agreement has taken effect on payment and has not been terminated under Section 11 — not on any technical control, and not on Licensee possessing a copy of the Certificate. A lost or delayed Certificate makes Licensee's rights harder to prove; it does not reduce them. The License ID is an identifier only; the Software does not read or check it.

---

## 8. Trademarks

No trademark, service mark, or logo right is granted under this Agreement, except as this Section states.

Licensee may (a) name "Aerini", in plain text, to accurately state that the Embedded Product includes or is built on the Software, and (b) give the Embedded Product its own name and branding, including in its user interface.

Licensee must not (i) use "Aerini" or the Aerini logo as the name or primary branding of the Embedded Product or of any other product or service, including a competing commercial product built on the Software; (ii) use them in a way that states or implies that Licensor makes, endorses, or supports the Embedded Product; or (iii) alter the Aerini logo or present it as Licensee's own mark.

**Credit.** The Embedded Product must state, in plain text, that it includes Aerini, in its About panel, legal-notices screen, or (if it has neither) its documentation, alongside the notices Section 4(a) requires. Licensee may not remove or obscure that credit.

Any use beyond this Section needs Licensor's written permission. Licensor's Trademark Policy is not part of this Agreement.

---

## 9. Transfer

This Agreement and the rights it grants are personal to Licensee and may not be assigned, sublicensed as a standalone license, or otherwise transferred without Licensor's prior written consent — except that Licensee may transfer this Agreement, without consent, to a successor that acquires substantially all of Licensee's business or assets (by merger, acquisition, or sale), provided the successor agrees in writing to be bound by this Agreement and Licensee notifies Licensor within 30 days of the transfer.

If Licensor sells, assigns, or otherwise transfers the Software or Licensor's rights in it, this Agreement transfers with it on its existing terms. Licensee's rights under this Agreement are not reduced by a change in who holds the Licensor role.

The limited end-customer sublicense granted in Section 3(b) is not a sublicense of this Agreement as a standalone license and is not restricted by this Section.

---

## 10. Fees

The fee is the one-time amount shown at checkout on the Certificate. It is not a subscription: no renewal or recurring payment is required to keep using the Licensed Version under this Agreement. Fees are non-refundable except as required by applicable law or as Licensor agrees in writing. Refund requests are handled by Polar as merchant of record under Polar's own refund policy; a chargeback is handled under the applicable card network's rules. See Section 11 for what happens to this Agreement if a payment is reversed.

---

## 11. Term and Termination

This Agreement remains in force for as long as Licensee complies with it. Licensor may terminate it if Licensee materially breaches it and does not cure the breach within 30 days of written notice — except a breach of Section 5 (AI/ML Training Restriction), which Licensor may terminate for immediately, without a cure period.

On termination, Licensee must stop distributing new copies of the Software under this Agreement's terms and, subject to the paragraph below, either revert to AGPL-3.0 for any continued use or stop using the Software entirely. Copies already delivered to Licensee's own end customers before termination are unaffected — those end customers may keep using what they already received. Reverting to AGPL-3.0 under this paragraph does not relieve Licensee of Section 5, which continues to bind Licensee under this Section's survival clause regardless of which license governs Licensee's continued use of the Software.

If the fee is reversed, charged back, or otherwise recovered by Licensee or its payment method after payment — for any reason other than Licensor's own refund under Section 10 — this Agreement terminates immediately and automatically, without notice or cure period, as of the date of the reversal.

If this Agreement is terminated other than for breach of Section 5, Licensee has 90 days from the date of termination to either (a) stop operating the Licensed Version, as modified by Licensee's Modifications, as a network-accessible service, or (b) release Modifications made after the termination date under AGPL-3.0. During those 90 days, Section 3(c) continues to apply to what Licensee had already deployed at the termination date, so no AGPL-3.0 disclosure duty attaches to it. After the 90 days: if Licensee has done (a), no AGPL-3.0 disclosure duty arises from the termination; if Licensee has done (b), Section 3(c) continues to apply to what was already deployed at the termination date, and AGPL-3.0 applies to Modifications made after that date; and if Licensee has done neither, AGPL-3.0 applies in full, including to what was already deployed. On termination for breach of Section 5, this paragraph does not apply and the preceding paragraph applies immediately.

Sections 5, 6, 12, 13, 14, and 17 survive termination.

---

## 12. Warranty Disclaimer

THE SOFTWARE IS PROVIDED "AS IS," WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING WITHOUT LIMITATION THE IMPLIED WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE, TITLE, AND NON-INFRINGEMENT. LICENSOR DOES NOT WARRANT THAT THE SOFTWARE WILL BE ERROR-FREE OR OPERATE UNINTERRUPTED.

---

## 13. Limitation of Liability

TO THE MAXIMUM EXTENT PERMITTED BY LAW, LICENSOR'S TOTAL LIABILITY ARISING OUT OF OR RELATED TO THIS AGREEMENT OR THE SOFTWARE WILL NOT EXCEED THE FEE LICENSEE ACTUALLY PAID UNDER THE CERTIFICATE. LICENSOR WILL NOT BE LIABLE FOR ANY INDIRECT, INCIDENTAL, SPECIAL, CONSEQUENTIAL, OR PUNITIVE DAMAGES, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGES. THIS LIMITATION DOES NOT APPLY TO LIABILITY ARISING FROM LICENSOR'S FRAUD OR WILFUL MISCONDUCT, OR TO THE EXTENT IT CANNOT BE EXCLUDED BY APPLICABLE LAW.

---

## 14. Indemnification

Licensee will indemnify, defend, and hold harmless Licensor against any third-party claim arising out of Licensee's Modifications, or Licensee's use or distribution of the Software in violation of this Agreement — except to the extent the claim arises from the unmodified Software itself.

---

## 15. Certificate

Licensor's checkout process issues the Certificate automatically when payment completes, as described in Section 1. The Certificate is evidence of this Agreement and of the license it grants; it is not itself the operative legal document — this Agreement is. Licensee should keep the order confirmation and license key it receives, and is responsible for entering its identity and the Embedded Product name accurately at checkout; Licensee may ask Licensor at panchak2d@aerini.org to correct an entry. Licensor may, at its discretion, provide a separate written certificate on request. Licensee's rights arise on the Effective Date; a delayed, lost, or unissued Certificate does not reduce them. If the Certificate and this Agreement conflict on anything other than Licensee's identity, the Licensed Version, the Embedded Product, or the License ID, this Agreement controls.

---

## 16. Export Compliance

Licensee is responsible for complying with any export control, sanctions, or trade law applicable to Licensee's own use or distribution of the Software.

---

## 17. Governing Law and Disputes

This Agreement is governed by the laws of India, without regard to conflict-of-law principles. Any dispute arising out of or related to this Agreement is subject to the exclusive jurisdiction of the courts having territorial jurisdiction over Licensor's principal place of business. Nothing in this Section prevents either party from seeking urgent interim relief from a court of competent jurisdiction to prevent irreparable harm.

---

## 18. General

**Entire Agreement.** This Agreement, together with the Certificate, is the entire agreement between the parties regarding the Licensed Version, and supersedes any prior discussion or understanding on the subject.

**Amendments.** Licensor may publish a new version of this Agreement for future sales. Doing so does not change the terms Licensee already accepted — Licensee remains governed by the version in force on Licensee's Effective Date unless Licensee separately agrees to a new version.

**Severability.** If a provision of this Agreement is held unenforceable, the rest of the Agreement remains in effect, and the unenforceable provision is modified to the minimum extent necessary to make it enforceable.

**No Waiver.** Licensor's failure to enforce a provision on one occasion is not a waiver of the right to enforce it later.

**Notices.** Notices to Licensor go to panchak2d@aerini.org. Notices to Licensee go to the email address used at checkout.

---

*Aerini Commercial License Agreement — Version 1.0*
*Panchaketu Debbarma — Aerini maintainer — https://github.com/Panchak2d/aerini*
