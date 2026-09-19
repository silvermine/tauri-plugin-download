//
//  DownloadsView.swift
//  DownloadManagerExample
//

import SwiftUI
import DownloadManagerKit

struct PendingDownload: Identifiable {
   var id: String { path }
   let url: URL
   let path: String
   let options: CreateOptions
}

struct DownloadsView: View {
   private let manager = DownloadManager.shared
   @State private var downloads: [DownloadItem] = []
   @State private var downloadURL: String = ""
   @State private var autoCreate: Bool = true
   @State private var allowMetered: Bool = true
   @State private var pendingDownloads: [PendingDownload] = []

   var body: some View {
      NavigationView {
         VStack {
            Text("Enter a URL to download and click Get.")
               .font(.subheadline)
               .foregroundColor(.secondary)
               .padding(.top)
            
            HStack {
               TextField("https://example.com/file.zip", text: $downloadURL)
                  .textFieldStyle(RoundedBorderTextFieldStyle())
                  .autocapitalization(.none)
                  .disableAutocorrection(true)
                  .keyboardType(.URL)
               
               Button(action: getDownload) {
                  Text("Get")
                     .padding(.horizontal, 10)
                     .padding(.vertical, 5)
                     .background(Color.blue)
                     .foregroundColor(.white)
                     .cornerRadius(8)
               }
               .disabled(downloadURL.isEmpty)
            }
            .padding(.horizontal)
            
            Toggle("Auto-create", isOn: $autoCreate)
               .padding(.horizontal)

            // Fixed on the download at creation. Turn this off and the transfer is
            // confined to unmetered, unconstrained networks — toggling Low Data Mode
            // on the current Wi-Fi network is the cheapest way to exercise it.
            Toggle("Allow metered networks", isOn: $allowMetered)
               .padding(.horizontal)

            List {
               ForEach(pendingDownloads) { pending in
                  PendingDownloadRowView(pending: pending, manager: manager, onCreated: {
                     pendingDownloads.removeAll { $0.path == pending.path }
                  })
               }
               ForEach(downloads, id: \.path) { item in
                  DownloadRowView(item: item, manager: manager)
               }
            }
         }
         .navigationTitle("Downloads")
         .task {
            downloads = await manager.list()
            for await download in manager.changed {
               print("[\(URL(fileURLWithPath: download.path).lastPathComponent)] \(download.status) - \(String(format: "%.0f", download.progress))% (\(download.receivedBytes)/\(download.totalBytes.map { String($0) } ?? "unknown") bytes)")
               downloads = await manager.list()
            }
         }
      }
   }

   private func getDownload() {
      guard !downloadURL.isEmpty,
            let url = URL(string: downloadURL),
            url.scheme != nil && url.host != nil else {
         return
      }

      let filename = url.lastPathComponent
      let path = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0].appendingPathComponent(filename).path
      
      // Read before the suspension point: the policy is fixed when the download is
      // listed, not re-read whenever the task resumes.
      let options = CreateOptions(allowMetered: allowMetered)

      Task {
         let download = await manager.get(path: path)
         
         if download == nil {
            if autoCreate {
               _ = await manager.create(path: path, url: url, options: options)
            } else {
               pendingDownloads.append(PendingDownload(url: url, path: path, options: options))
            }
         }
      }
      
      downloadURL = ""
   }
}

struct PendingDownloadRowView: View {
   let pending: PendingDownload
   let manager: DownloadManager
   let onCreated: () -> Void
   
   var body: some View {
      VStack(alignment: .leading) {
         Text(URL(fileURLWithPath: pending.path).lastPathComponent)
            .font(.headline)
         Text("Status: pending")
            .font(.caption)
            .foregroundColor(.secondary)
         
         Button(action: {
            Task {
               _ = await manager.create(path: pending.path, url: pending.url, options: pending.options)
               onCreated()
            }
         }) {
            Text("Create")
               .padding(8)
               .background(Color.green.opacity(0.2))
               .cornerRadius(8)
         }.buttonStyle(PlainButtonStyle())
      }
      .padding(.vertical, 4)
   }
}

struct DownloadRowView: View {
   let item: DownloadItem
   let manager: DownloadManager
   
   /// A server that omits the content length leaves `totalBytes` nil, so the
   /// total is not always known even while bytes are arriving.
   private var byteCount: String {
      let formatter = ByteCountFormatter()
      let received = formatter.string(fromByteCount: Int64(item.receivedBytes))
      let total = item.totalBytes.map { formatter.string(fromByteCount: Int64($0)) } ?? "unknown"

      return "\(received) / \(total)"
   }

   var body: some View {
      VStack(alignment: .leading) {
         Text(URL(fileURLWithPath: item.path).lastPathComponent)
            .font(.headline)
         ProgressView(value: item.progress / 100)
            .progressViewStyle(LinearProgressViewStyle())
         Text("Status: \(item.status.rawValue) — \(String(format: "%.0f", item.progress))%")
            .font(.caption)
            .foregroundColor(.secondary)
         Text(byteCount)
            .font(.caption)
            .foregroundColor(.secondary)
         Text(item.options.allowMetered ? "Metered: allowed" : "Metered: blocked")
            .font(.caption)
            .foregroundColor(item.options.allowMetered ? .secondary : .orange)
         
         switch item.status {
         case .idle:
            HStack(spacing: 8) {
               Button(action: { Task { _ = try? await manager.start(path: item.path) } }) {
                  Text("Start")
                     .padding(8)
                     .background(Color.blue.opacity(0.2))
                     .cornerRadius(8)
               }.buttonStyle(PlainButtonStyle())
               Button(action: { Task { _ = try? await manager.cancel(path: item.path) } }) {
                  Text("Cancel")
                     .padding(8)
                     .background(Color.red.opacity(0.2))
                     .cornerRadius(8)
               }.buttonStyle(PlainButtonStyle())
            }
         case .inProgress:
            HStack(spacing: 8) {
               Button(action: { Task { _ = try? await manager.pause(path: item.path) } }) {
                  Text("Pause")
                     .padding(8)
                     .background(Color.blue.opacity(0.2))
                     .cornerRadius(8)
               }.buttonStyle(PlainButtonStyle())
               Button(action: { Task { _ = try? await manager.cancel(path: item.path) } }) {
                  Text("Cancel")
                     .padding(8)
                     .background(Color.red.opacity(0.2))
                     .cornerRadius(8)
               }.buttonStyle(PlainButtonStyle())
            }
         case .paused:
            HStack(spacing: 8) {
               Button(action: { Task { _ = try? await manager.resume(path: item.path) } }) {
                  Text("Resume")
                     .padding(8)
                     .background(Color.blue.opacity(0.2))
                     .cornerRadius(8)
               }.buttonStyle(PlainButtonStyle())
               Button(action: { Task { _ = try? await manager.cancel(path: item.path) } }) {
                  Text("Cancel")
                     .padding(8)
                     .background(Color.red.opacity(0.2))
                     .cornerRadius(8)
               }.buttonStyle(PlainButtonStyle())
            }
         default:
            EmptyView()
         }
      }
      .padding(.vertical, 4)
   }
}
