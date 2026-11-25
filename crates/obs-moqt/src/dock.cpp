// The MoQT dock: edits the `moqt_service` settings and makes that service
// OBS's stream destination. OBS's own Settings -> Stream page lists only its
// built-in services, so this is where a user enters them.
//
// The class has no signals or slots of its own, so the file needs no moc.

// Qt 6.8 calls `__yield()` on Arm without declaring it, which recent clang
// rejects; this header declares it.
#if defined(__aarch64__) && __has_include(<arm_acle.h>)
#include <arm_acle.h>
#endif

#include <QtWidgets/QCheckBox>
#include <QtWidgets/QFormLayout>
#include <QtWidgets/QLabel>
#include <QtWidgets/QLineEdit>
#include <QtWidgets/QPushButton>
#include <QtWidgets/QWidget>

// The part of libobs and obs-frontend-api this file uses, declared by hand
// from the OBS Studio 32.0 headers (`obs.h`, `obs-data.h`,
// `obs-frontend-api.h`) so that the build needs no OBS sources.
extern "C" {
struct obs_data;
struct obs_service;

obs_data *obs_data_create();
void obs_data_release(obs_data *data);
void obs_data_set_string(obs_data *data, const char *name, const char *value);
void obs_data_set_bool(obs_data *data, const char *name, bool value);
const char *obs_data_get_string(obs_data *data, const char *name);
bool obs_data_get_bool(obs_data *data, const char *name);

obs_service *obs_service_create(const char *id, const char *name, obs_data *settings, obs_data *hotkey_data);
void obs_service_release(obs_service *service);
const char *obs_service_get_id(const obs_service *service);
// Returns a new reference.
obs_data *obs_service_get_settings(const obs_service *service);

bool obs_frontend_add_dock_by_id(const char *id, const char *title, void *widget);
void obs_frontend_add_event_callback(void (*callback)(int event, void *data), void *data);
void obs_frontend_remove_event_callback(void (*callback)(int event, void *data), void *data);
// Returns a borrowed reference.
obs_service *obs_frontend_get_streaming_service();
void obs_frontend_set_streaming_service(obs_service *service);
void obs_frontend_save_streaming_service();
bool obs_frontend_streaming_active();
}

namespace {

// `enum obs_frontend_event`.
constexpr int OBS_FRONTEND_EVENT_PROFILE_CHANGED = 15;
constexpr int OBS_FRONTEND_EVENT_FINISHED_LOADING = 26;

constexpr const char *SERVICE_ID = "moqt_service";

class MoqtDock : public QWidget {
public:
	MoqtDock()
	{
		token->setEchoMode(QLineEdit::Password);
		server->setPlaceholderText("moqt://relay.example.com:443");
		name_space->setPlaceholderText("obs/live");
		token_file->setPlaceholderText("Instead of a token");

		auto *layout = new QFormLayout(this);
		layout->addRow("Relay URL", server);
		layout->addRow("Namespace", name_space);
		layout->addRow("Token", token);
		layout->addRow("Token file", token_file);
		layout->addRow(skip_tls_verify);
		layout->addRow(use);
		layout->addRow(status);

		connect(use, &QPushButton::clicked, this, [this] { use_for_streaming(); });
		obs_frontend_add_event_callback(on_frontend_event, this);
	}

	~MoqtDock() override { obs_frontend_remove_event_callback(on_frontend_event, this); }

private:
	QLineEdit *server = new QLineEdit;
	QLineEdit *name_space = new QLineEdit;
	QLineEdit *token = new QLineEdit;
	QLineEdit *token_file = new QLineEdit;
	QCheckBox *skip_tls_verify = new QCheckBox("Skip TLS verification");
	QPushButton *use = new QPushButton("Use for streaming");
	QLabel *status = new QLabel;

	static void on_frontend_event(int event, void *data)
	{
		if (event == OBS_FRONTEND_EVENT_FINISHED_LOADING || event == OBS_FRONTEND_EVENT_PROFILE_CHANGED)
			static_cast<MoqtDock *>(data)->load();
	}

	// Shows the profile's stream destination: its settings when it is MoQT.
	void load()
	{
		obs_service *service = obs_frontend_get_streaming_service();
		bool is_moqt = service && qstrcmp(obs_service_get_id(service), SERVICE_ID) == 0;
		status->setText(is_moqt ? "MoQT is the stream destination" : "MoQT is not the stream destination");
		if (!is_moqt)
			return;
		obs_data *settings = obs_service_get_settings(service);
		server->setText(obs_data_get_string(settings, "server"));
		name_space->setText(obs_data_get_string(settings, "namespace"));
		token->setText(obs_data_get_string(settings, "bearer_token"));
		token_file->setText(obs_data_get_string(settings, "bearer_token_file"));
		skip_tls_verify->setChecked(obs_data_get_bool(settings, "insecure_skip_tls_verify"));
		obs_data_release(settings);
	}

	// OBS keeps the service of a running stream, so the destination changes
	// only between streams.
	void use_for_streaming()
	{
		if (obs_frontend_streaming_active()) {
			status->setText("Stop streaming to change the destination");
			return;
		}
		obs_data *settings = obs_data_create();
		obs_data_set_string(settings, "server", server->text().trimmed().toUtf8().constData());
		obs_data_set_string(settings, "namespace", name_space->text().trimmed().toUtf8().constData());
		obs_data_set_string(settings, "bearer_token", token->text().trimmed().toUtf8().constData());
		obs_data_set_string(settings, "bearer_token_file", token_file->text().trimmed().toUtf8().constData());
		obs_data_set_bool(settings, "insecure_skip_tls_verify", skip_tls_verify->isChecked());
		obs_service *service = obs_service_create(SERVICE_ID, "default_service", settings, nullptr);
		obs_data_release(settings);
		obs_frontend_set_streaming_service(service);
		obs_frontend_save_streaming_service();
		obs_service_release(service);
		load();
	}
};

} // namespace

// Called once from `obs_module_load`, on OBS's UI thread. OBS owns the widget.
extern "C" void obs_moqt_add_dock()
{
	obs_frontend_add_dock_by_id("obs-moqt-dock", "MoQT", new MoqtDock);
}
