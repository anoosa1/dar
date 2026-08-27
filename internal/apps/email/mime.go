package email

import (
	"bufio"
	"bytes"
	"crypto/sha256"
	"encoding/base64"
	"encoding/hex"
	"errors"
	"fmt"
	"io"
	"mime"
	"mime/multipart"
	"mime/quotedprintable"
	"net/mail"
	"net/textproto"
	"os"
	"regexp"
	"strconv"
	"strings"
	"time"
	"unicode/utf8"
)

type parsedRecipient struct {
	Kind, Name, Email string
	Ordinal           int
}
type parsedAttachment struct {
	Attachment
	Data []byte
}
type parsedMail struct {
	Hash, HeaderMessageID, InReplyTo, References, ThreadKey string
	From, To, Cc, Bcc, Subject, Snippet, Text, HTML         string
	Date                                                    int64
	Size                                                    int64
	Recipients                                              []parsedRecipient
	Attachments                                             []parsedAttachment
	ParseError                                              string
}

var messageIDRE = regexp.MustCompile(`<[^<>\s]+>`)

func decodeHeader(v string) string {
	d := &mime.WordDecoder{CharsetReader: func(charset string, input io.Reader) (io.Reader, error) {
		b, err := io.ReadAll(input)
		if err != nil {
			return nil, err
		}
		return bytes.NewReader(decodeCharset(b, charset)), nil
	}}
	if s, err := d.DecodeHeader(v); err == nil {
		return strings.TrimSpace(s)
	}
	return strings.TrimSpace(v)
}

func decodeCharset(b []byte, charset string) []byte {
	switch strings.ToLower(strings.TrimSpace(charset)) {
	case "", "utf-8", "utf8", "us-ascii":
		if utf8.Valid(b) {
			return b
		}
		return []byte(strings.ToValidUTF8(string(b), "�"))
	case "iso-8859-1", "latin1", "latin-1":
		var out strings.Builder
		for _, c := range b {
			out.WriteRune(rune(c))
		}
		return []byte(out.String())
	case "windows-1252", "cp1252":
		cp := map[byte]rune{0x80: '€', 0x82: '‚', 0x83: 'ƒ', 0x84: '„', 0x85: '…', 0x86: '†', 0x87: '‡', 0x88: 'ˆ', 0x89: '‰', 0x8a: 'Š', 0x8b: '‹', 0x8c: 'Œ', 0x8e: 'Ž', 0x91: '‘', 0x92: '’', 0x93: '“', 0x94: '”', 0x95: '•', 0x96: '–', 0x97: '—', 0x98: '˜', 0x99: '™', 0x9a: 'š', 0x9b: '›', 0x9c: 'œ', 0x9e: 'ž', 0x9f: 'Ÿ'}
		var out strings.Builder
		for _, c := range b {
			if r, ok := cp[c]; ok {
				out.WriteRune(r)
			} else {
				out.WriteRune(rune(c))
			}
		}
		return []byte(out.String())
	default:
		return []byte(strings.ToValidUTF8(string(b), "�"))
	}
}

func decodeTransfer(r io.Reader, encoding string) io.Reader {
	switch strings.ToLower(strings.TrimSpace(encoding)) {
	case "base64":
		return base64.NewDecoder(base64.StdEncoding, r)
	case "quoted-printable":
		return quotedprintable.NewReader(r)
	default:
		return r
	}
}

func parseAddresses(kind string, h mail.Header, start int) ([]parsedRecipient, string) {
	var out []parsedRecipient
	var labels []string
	values := h.Get(kind)
	if values == "" {
		return out, ""
	}
	list, err := mail.ParseAddressList(values)
	if err != nil {
		return out, decodeHeader(values)
	}
	for i, a := range list {
		name := decodeHeader(a.Name)
		out = append(out, parsedRecipient{Kind: strings.ToLower(kind), Name: name, Email: strings.ToLower(a.Address), Ordinal: start + i})
		if name != "" {
			labels = append(labels, name+" <"+a.Address+">")
		} else {
			labels = append(labels, a.Address)
		}
	}
	return out, strings.Join(labels, ", ")
}

func parseMailFile(path string) (parsedMail, error) {
	data, err := os.ReadFile(path)
	if err != nil {
		return parsedMail{}, err
	}
	p := parsedMail{Size: int64(len(data))}
	sum := sha256.Sum256(data)
	p.Hash = hex.EncodeToString(sum[:])
	msg, err := mail.ReadMessage(bytes.NewReader(data))
	if err != nil {
		p.ParseError = err.Error()
		p.ThreadKey = p.Hash
		return p, nil
	}
	p.HeaderMessageID = strings.TrimSpace(msg.Header.Get("Message-ID"))
	p.InReplyTo = strings.TrimSpace(msg.Header.Get("In-Reply-To"))
	p.References = strings.TrimSpace(msg.Header.Get("References"))
	p.Subject = decodeHeader(msg.Header.Get("Subject"))
	p.From = decodeHeader(msg.Header.Get("From"))
	p.Date = parseMailDate(msg.Header.Get("Date"))
	if p.Date == 0 {
		if fi, e := os.Stat(path); e == nil {
			p.Date = fi.ModTime().Unix()
		}
	}
	to, toLabel := parseAddresses("To", msg.Header, 0)
	cc, ccLabel := parseAddresses("Cc", msg.Header, len(to))
	bcc, bccLabel := parseAddresses("Bcc", msg.Header, len(to)+len(cc))
	from, _ := parseAddresses("From", msg.Header, len(to)+len(cc)+len(bcc))
	p.Recipients = append(p.Recipients, from...)
	p.Recipients = append(p.Recipients, to...)
	p.Recipients = append(p.Recipients, cc...)
	p.Recipients = append(p.Recipients, bcc...)
	p.To = toLabel
	p.Cc = ccLabel
	p.Bcc = bccLabel
	refs := messageIDRE.FindAllString(p.References, -1)
	if len(refs) > 0 {
		p.ThreadKey = refs[0]
	} else if reply := messageIDRE.FindString(p.InReplyTo); reply != "" {
		p.ThreadKey = reply
	} else if p.HeaderMessageID != "" {
		p.ThreadKey = p.HeaderMessageID
	} else {
		p.ThreadKey = p.Hash
	}
	mediaType, params, _ := mime.ParseMediaType(msg.Header.Get("Content-Type"))
	if mediaType == "" {
		mediaType = "text/plain"
	}
	if err := parseMIMEPart(textproto.MIMEHeader(msg.Header), msg.Body, mediaType, params, "1", &p); err != nil {
		p.ParseError = err.Error()
	}
	p.Text = strings.TrimSpace(p.Text)
	p.HTML = strings.TrimSpace(p.HTML)
	if p.Text == "" && p.HTML != "" {
		p.Text = strings.TrimSpace(stripHTML(p.HTML))
	}
	source := p.Text
	source = strings.Join(strings.Fields(source), " ")
	if len([]rune(source)) > 220 {
		source = string([]rune(source)[:220]) + "…"
	}
	p.Snippet = source
	return p, nil
}

func parseMIMEPart(h textproto.MIMEHeader, r io.Reader, mediaType string, params map[string]string, part string, p *parsedMail) error {
	if strings.HasPrefix(mediaType, "multipart/") {
		boundary := params["boundary"]
		if boundary == "" {
			return errors.New("multipart message has no boundary")
		}
		mr := multipart.NewReader(r, boundary)
		idx := 1
		for {
			child, err := mr.NextPart()
			if err == io.EOF {
				break
			}
			if err != nil {
				return err
			}
			ct, cp, _ := mime.ParseMediaType(child.Header.Get("Content-Type"))
			if ct == "" {
				ct = "text/plain"
			}
			if err := parseMIMEPart(child.Header, child, ct, cp, part+"."+strconv.Itoa(idx), p); err != nil {
				return err
			}
			idx++
		}
		return nil
	}
	body, err := io.ReadAll(io.LimitReader(decodeTransfer(r, h.Get("Content-Transfer-Encoding")), 64<<20))
	if err != nil {
		return err
	}
	disp, dp, _ := mime.ParseMediaType(h.Get("Content-Disposition"))
	filename := decodeHeader(dp["filename"])
	if filename == "" {
		filename = decodeHeader(params["name"])
	}
	cid := strings.Trim(strings.TrimSpace(h.Get("Content-ID")), "<>")
	isAttachment := disp == "attachment" || filename != "" || cid != ""
	if isAttachment {
		if filename == "" {
			filename = "attachment-" + part
		}
		p.Attachments = append(p.Attachments, parsedAttachment{Attachment: Attachment{Part: part, Filename: filename, MIMEType: mediaType, Size: int64(len(body)), ContentID: cid, Inline: disp == "inline" || cid != ""}, Data: body})
		return nil
	}
	body = decodeCharset(body, params["charset"])
	switch mediaType {
	case "text/plain":
		if p.Text != "" {
			p.Text += "\n\n"
		}
		p.Text += string(body)
	case "text/html":
		if p.HTML != "" {
			p.HTML += "<hr>"
		}
		p.HTML += string(body)
	}
	return nil
}

func parseMailDate(v string) int64 {
	if t, err := mail.ParseDate(v); err == nil {
		return t.Unix()
	}
	for _, f := range []string{time.RFC1123Z, time.RFC822Z, time.RFC3339} {
		if t, err := time.Parse(f, v); err == nil {
			return t.Unix()
		}
	}
	return 0
}

var htmlTagRE = regexp.MustCompile(`(?s)<[^>]*>`)

func stripHTML(s string) string { return htmlTagRE.ReplaceAllString(s, " ") }

func extractPart(path, want string) (parsedAttachment, error) {
	data, err := os.ReadFile(path)
	if err != nil {
		return parsedAttachment{}, err
	}
	msg, err := mail.ReadMessage(bytes.NewReader(data))
	if err != nil {
		return parsedAttachment{}, err
	}
	ct, params, _ := mime.ParseMediaType(msg.Header.Get("Content-Type"))
	p := parsedMail{}
	if err := parseMIMEPart(textproto.MIMEHeader(msg.Header), msg.Body, ct, params, "1", &p); err != nil {
		return parsedAttachment{}, err
	}
	for _, a := range p.Attachments {
		if a.Part == want {
			return a, nil
		}
	}
	return parsedAttachment{}, errNotFound
}

func writeBase64Lines(w io.Writer, data []byte) error {
	enc := base64.StdEncoding.EncodeToString(data)
	bw := bufio.NewWriter(w)
	for len(enc) > 76 {
		if _, err := bw.WriteString(enc[:76] + "\r\n"); err != nil {
			return err
		}
		enc = enc[76:]
	}
	if _, err := bw.WriteString(enc + "\r\n"); err != nil {
		return err
	}
	return bw.Flush()
}

func validHeaderValue(v string) bool { return !strings.ContainsAny(v, "\r\n") }
func quoteFilename(v string) string {
	return strings.ReplaceAll(strings.ReplaceAll(v, "\\", "_"), `"`, `_`)
}
func formatMessageID(id string) string {
	if strings.HasPrefix(id, "<") && strings.HasSuffix(id, ">") {
		return id
	}
	return fmt.Sprintf("<%s>", id)
}
